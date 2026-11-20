//! Unidades de armazenamento fora do Windows.
//!
//! - Linux: pontos de montagem reais lidos de `/proc/self/mountinfo`, sem
//!   pseudo-sistemas de arquivos (proc, sysfs, tmpfs, cgroup, overlay do Docker,
//!   squashfs de snap, FUSE de portal...), sem montagens de rede e sem bind
//!   mounts repetidos. Capacidade via `statvfs`.
//! - macOS: volumes do `sysinfo` (TODO(macos): validar em hardware).

use super::DriveInfo;
#[cfg(target_os = "linux")]
use std::collections::HashMap;
use std::path::Path;
#[cfg(target_os = "linux")]
use std::path::PathBuf;

/// `(total, livre)` em bytes do sistema de arquivos que contém `path`.
/// "Livre" é o espaço que um usuário comum ainda pode gravar (`f_bavail`,
/// o mesmo "Disponível" do `df`), sem os blocos reservados ao root.
pub(crate) fn capacity(path: &Path) -> Option<(u64, u64)> {
    use std::os::unix::ffi::OsStrExt;
    let c_path = std::ffi::CString::new(path.as_os_str().as_bytes()).ok()?;
    // SAFETY: `statvfs` é POD; zero é um valor inicial válido e a chamada só
    // escreve no buffer passado. O caminho é uma C string terminada em NUL.
    let mut stats: libc::statvfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statvfs(c_path.as_ptr(), &mut stats) } != 0 {
        return None;
    }
    #[allow(clippy::unnecessary_cast)] // os tipos variam entre Linux e macOS
    let fragment = if stats.f_frsize > 0 {
        stats.f_frsize as u64
    } else {
        stats.f_bsize as u64
    };
    #[allow(clippy::unnecessary_cast)]
    let total = (stats.f_blocks as u64).checked_mul(fragment)?;
    #[allow(clippy::unnecessary_cast)]
    let free = (stats.f_bavail as u64).saturating_mul(fragment);
    (total > 0).then_some((total, free))
}

/// Uma linha de `/proc/self/mountinfo`.
#[cfg(target_os = "linux")]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MountEntry {
    /// `major:minor` do dispositivo.
    pub(crate) device: String,
    /// Raiz da montagem dentro do sistema de arquivos (`/` exceto em bind mounts
    /// e subvolumes btrfs).
    pub(crate) root: String,
    pub(crate) mount_point: String,
    pub(crate) fs_type: String,
    pub(crate) source: String,
}

/// O kernel escapa espaço, tab, `\n` e `\` como `\040`, `\011`, `\012`, `\134`.
#[cfg(target_os = "linux")]
fn unescape_mount_field(raw: &str) -> String {
    let bytes = raw.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'\\' {
            if let Some(value) = raw
                .get(index + 1..index + 4)
                .filter(|digits| digits.bytes().all(|b| (b'0'..=b'7').contains(&b)))
                .and_then(|digits| u8::from_str_radix(digits, 8).ok())
            {
                out.push(value);
                index += 4;
                continue;
            }
        }
        out.push(bytes[index]);
        index += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(target_os = "linux")]
pub(crate) fn parse_mountinfo(text: &str) -> Vec<MountEntry> {
    text.lines()
        .filter_map(|line| {
            let (left, right) = line.split_once(" - ")?;
            let left: Vec<&str> = left.split(' ').collect();
            let mut right = right.split(' ');
            if left.len() < 5 {
                return None;
            }
            Some(MountEntry {
                device: left[2].to_owned(),
                root: unescape_mount_field(left[3]),
                mount_point: unescape_mount_field(left[4]),
                fs_type: right.next()?.to_owned(),
                source: unescape_mount_field(right.next().unwrap_or_default()),
            })
        })
        .collect()
}

/// Sistemas de arquivos que não são armazenamento do usuário: virtuais do
/// kernel, memória, camadas de contêiner, imagens somente leitura (snap, ISO)
/// e montagens de rede (o Windows também esconde unidades de rede e de CD).
#[cfg(target_os = "linux")]
const IGNORED_FS_TYPES: &[&str] = &[
    "proc",
    "sysfs",
    "tmpfs",
    "devtmpfs",
    "devpts",
    "ramfs",
    "cgroup",
    "cgroup2",
    "overlay",
    "aufs",
    "squashfs",
    "iso9660",
    "udf",
    "securityfs",
    "pstore",
    "bpf",
    "tracefs",
    "debugfs",
    "configfs",
    "fusectl",
    "mqueue",
    "hugetlbfs",
    "autofs",
    "binfmt_misc",
    "efivarfs",
    "nsfs",
    "rpc_pipefs",
    "nfsd",
    "selinuxfs",
    "fuse.portal",
    "fuse.gvfsd-fuse",
    "fuse.snapfuse",
    "fuse.lxcfs",
    "fuse.sshfs",
    "fuse.rclone",
    "nfs",
    "nfs4",
    "cifs",
    "smb3",
    "smbfs",
    "9p",
    "ceph",
    "glusterfs",
];

/// Árvores de pontos de montagem que nunca são unidades do usuário.
#[cfg(target_os = "linux")]
const IGNORED_MOUNT_PREFIXES: &[&str] = &[
    "/proc",
    "/sys",
    "/dev",
    "/run",
    "/snap",
    "/var/snap",
    "/var/lib/docker",
    "/var/lib/containers",
    "/var/lib/kubelet",
    "/boot/efi",
    "/efi",
    "/tmp/.mount_",
];

#[cfg(target_os = "linux")]
fn under(path: &str, prefix: &str) -> bool {
    path == prefix
        || path
            .strip_prefix(prefix)
            .is_some_and(|rest| rest.starts_with('/') || prefix.ends_with('_'))
}

#[cfg(target_os = "linux")]
fn is_user_volume(entry: &MountEntry) -> bool {
    if IGNORED_FS_TYPES.contains(&entry.fs_type.as_str()) {
        return false;
    }
    // Pendrives e discos externos montados pelo udisks ficam em
    // `/run/media/<usuário>` (Fedora/Arch) ou `/media/<usuário>` (Debian/Ubuntu).
    if under(&entry.mount_point, "/run/media") {
        return true;
    }
    !IGNORED_MOUNT_PREFIXES
        .iter()
        .any(|prefix| under(&entry.mount_point, prefix))
}

/// Filtra, remove bind mounts de pastas de um volume já listado e ordena:
/// `/` primeiro, depois por ponto de montagem.
#[cfg(target_os = "linux")]
pub(crate) fn user_volumes(entries: Vec<MountEntry>) -> Vec<MountEntry> {
    let candidates: Vec<MountEntry> = entries.into_iter().filter(is_user_volume).collect();
    let whole_devices: Vec<&str> = candidates
        .iter()
        .filter(|entry| entry.root == "/")
        .map(|entry| entry.device.as_str())
        .collect();
    let mut seen_points = std::collections::HashSet::new();
    let mut volumes: Vec<MountEntry> = candidates
        .iter()
        // Bind mount de uma subpasta de um volume que já aparece inteiro.
        // Subvolumes btrfs (`/@`, `/@home`) não têm entrada com raiz `/` e
        // continuam listados.
        .filter(|entry| entry.root == "/" || !whole_devices.contains(&entry.device.as_str()))
        .filter(|entry| seen_points.insert(entry.mount_point.clone()))
        .cloned()
        .collect();
    volumes.sort_by(|a, b| {
        (a.mount_point != "/")
            .cmp(&(b.mount_point != "/"))
            .then_with(|| a.mount_point.cmp(&b.mount_point))
    });
    volumes
}

/// Volumes reais montados agora. Lista vazia quando `/proc` não pôde ser lido.
#[cfg(target_os = "linux")]
pub(crate) fn mounted_user_volumes() -> Vec<MountEntry> {
    std::fs::read_to_string("/proc/self/mountinfo")
        .map(|text| user_volumes(parse_mountinfo(&text)))
        .unwrap_or_default()
}

/// Rótulos de `/dev/disk/by-label` indexados pelo dispositivo real.
#[cfg(target_os = "linux")]
fn device_labels() -> HashMap<PathBuf, String> {
    let Ok(entries) = std::fs::read_dir("/dev/disk/by-label") else {
        return HashMap::new();
    };
    entries
        .flatten()
        .filter_map(|entry| {
            let device = std::fs::canonicalize(entry.path()).ok()?;
            let label = unescape_udev_label(&entry.file_name().to_string_lossy());
            (!label.trim().is_empty()).then_some((device, label))
        })
        .collect()
}

/// O udev escapa caracteres especiais do rótulo como `\x20`.
#[cfg(target_os = "linux")]
fn unescape_udev_label(raw: &str) -> String {
    let bytes = raw.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'\\' && bytes.get(index + 1) == Some(&b'x') {
            if let Some(value) = raw
                .get(index + 2..index + 4)
                .and_then(|hex| u8::from_str_radix(hex, 16).ok())
            {
                out.push(value);
                index += 4;
                continue;
            }
        }
        out.push(bytes[index]);
        index += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(target_os = "linux")]
pub(crate) fn drive_label(mount_point: &str, device_label: Option<&str>) -> String {
    if let Some(label) = device_label.filter(|label| !label.trim().is_empty()) {
        return format!("{label} ({mount_point})");
    }
    match mount_point {
        "/" => "Sistema (/)".to_owned(),
        "/home" => "Pastas pessoais (/home)".to_owned(),
        other => {
            let name = Path::new(other)
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_else(|| other.to_owned());
            format!("{name} ({other})")
        }
    }
}

#[cfg(target_os = "linux")]
pub(crate) fn system_drives() -> Vec<DriveInfo> {
    let labels = device_labels();
    let mut drives: Vec<DriveInfo> = mounted_user_volumes()
        .into_iter()
        .map(|entry| {
            let device = std::fs::canonicalize(&entry.source).ok();
            let label = device.as_ref().and_then(|device| labels.get(device));
            let capacity = capacity(Path::new(&entry.mount_point));
            DriveInfo {
                label: drive_label(&entry.mount_point, label.map(String::as_str)),
                path: entry.mount_point,
                total_bytes: capacity.map(|(total, _)| total),
                free_bytes: capacity.map(|(_, free)| free),
            }
        })
        .collect();
    if drives.is_empty() {
        drives.push(root_fallback());
    }
    drives
}

/// TODO(macos): validar em hardware. `/` é o volume de sistema selado; os
/// dados do usuário ficam em `/System/Volumes/Data`, que divide o mesmo
/// contêiner APFS (a capacidade é a mesma). Volumes externos ficam em `/Volumes`.
#[cfg(target_os = "macos")]
pub(crate) fn system_drives() -> Vec<DriveInfo> {
    let disks = sysinfo::Disks::new_with_refreshed_list();
    let mut drives: Vec<DriveInfo> = disks
        .list()
        .iter()
        .filter_map(|disk| {
            let mount_point = disk.mount_point().to_string_lossy().into_owned();
            if mount_point != "/" && !mount_point.starts_with("/Volumes/") {
                return None;
            }
            let total = disk.total_space();
            let name = disk.name().to_string_lossy().into_owned();
            let label = if mount_point == "/" {
                "Macintosh HD (/)".to_owned()
            } else if name.trim().is_empty() {
                format!("Volume ({mount_point})")
            } else {
                format!("{name} ({mount_point})")
            };
            Some(DriveInfo {
                path: mount_point,
                label,
                total_bytes: (total > 0).then_some(total),
                free_bytes: (total > 0).then_some(disk.available_space()),
            })
        })
        .collect();
    if drives.is_empty() {
        drives.push(root_fallback());
    }
    drives
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
pub(crate) fn system_drives() -> Vec<DriveInfo> {
    vec![root_fallback()]
}

/// Lista exibida no seletor: a pasta pessoal primeiro (raiz padrão da
/// varredura, como o `%USERPROFILE%` no Windows; varrer `/` inteiro é lento e
/// quase tudo ali é do sistema), depois os volumes. A capacidade da pasta
/// pessoal é a do volume que a contém.
pub(crate) fn drives_with_home() -> Vec<DriveInfo> {
    let mut drives = system_drives();
    if let Some(home) = dirs::home_dir().filter(|home| home.is_dir()) {
        let path = home.to_string_lossy().into_owned();
        if !drives.iter().any(|drive| drive.path == path) {
            let capacity = capacity(&home);
            drives.insert(
                0,
                DriveInfo {
                    label: format!("Pasta pessoal ({path})"),
                    path,
                    total_bytes: capacity.map(|(total, _)| total),
                    free_bytes: capacity.map(|(_, free)| free),
                },
            );
        }
    }
    drives
}

/// A raiz sempre existe; a capacidade só aparece se o `statvfs` responder.
fn root_fallback() -> DriveInfo {
    let capacity = capacity(Path::new("/"));
    DriveInfo {
        path: "/".to_owned(),
        label: "Sistema (/)".to_owned(),
        total_bytes: capacity.map(|(total, _)| total),
        free_bytes: capacity.map(|(_, free)| free),
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;

    const MOUNTINFO: &str = "\
22 1 259:2 / / rw,relatime shared:1 - ext4 /dev/nvme0n1p2 rw,errors=remount-ro
23 22 0:21 / /proc rw,nosuid,nodev,noexec,relatime shared:12 - proc proc rw
24 22 0:22 / /sys rw,nosuid,nodev,noexec,relatime shared:2 - sysfs sysfs rw
25 22 0:5 / /dev rw,nosuid,relatime shared:8 - devtmpfs udev rw,size=16G
26 25 0:23 / /dev/pts rw,nosuid,noexec,relatime shared:9 - devpts devpts rw
27 22 0:24 / /run rw,nosuid,nodev,noexec,relatime shared:10 - tmpfs tmpfs rw
28 24 0:25 / /sys/fs/cgroup rw,nosuid,nodev,noexec,relatime shared:4 - cgroup2 cgroup2 rw
29 22 259:4 / /boot/efi rw,relatime shared:30 - vfat /dev/nvme1n1p1 rw
30 22 0:62 / /var/lib/docker/overlay2/abc/merged rw,relatime - overlay overlay rw
31 22 7:3 / /snap/core22/1380 ro,nodev,relatime shared:40 - squashfs /dev/loop3 ro
32 22 0:40 / /tmp rw,nosuid,nodev shared:41 - tmpfs tmpfs rw
33 22 8:17 / /media/user/Meu\\040Pendrive rw,nosuid,nodev,relatime shared:50 - exfat /dev/sdb1 rw
34 27 8:33 / /run/media/user/Backup rw,nosuid,nodev,relatime shared:51 - ext4 /dev/sdc1 rw
35 22 259:2 /srv/dados /mnt/bind rw,relatime shared:1 - ext4 /dev/nvme0n1p2 rw
36 22 0:45 / /run/user/1000/doc rw,nosuid,nodev,relatime shared:60 - fuse.portal portal rw
37 22 0:46 /@home /home rw,relatime shared:61 - btrfs /dev/sda2 rw
38 22 0:46 /@data /dados rw,relatime shared:62 - btrfs /dev/sda2 rw
39 22 0:50 / /mnt/nas rw,relatime shared:63 - nfs4 nas:/export rw
40 22 0:51 / /tmp/.mount_AppXyz ro,nosuid,nodev,relatime - fuse.App.AppImage App.AppImage ro
";

    #[test]
    fn mountinfo_keeps_only_real_user_volumes() {
        let volumes = user_volumes(parse_mountinfo(MOUNTINFO));
        let points: Vec<&str> = volumes.iter().map(|v| v.mount_point.as_str()).collect();
        assert_eq!(
            points,
            vec![
                "/",
                "/dados",
                "/home",
                "/media/user/Meu Pendrive",
                "/run/media/user/Backup"
            ]
        );
    }

    #[test]
    fn mountinfo_decodes_octal_escapes() {
        let entries = parse_mountinfo(MOUNTINFO);
        let pendrive = entries
            .iter()
            .find(|entry| entry.fs_type == "exfat")
            .expect("pendrive");
        assert_eq!(pendrive.mount_point, "/media/user/Meu Pendrive");
        assert_eq!(pendrive.source, "/dev/sdb1");
        assert_eq!(unescape_mount_field("a\\134b\\011c"), "a\\b\tc");
        assert_eq!(unescape_mount_field("fim\\04"), "fim\\04");
    }

    #[test]
    fn mountinfo_ignores_malformed_lines() {
        assert!(parse_mountinfo("lixo\n1 2 3\n").is_empty());
    }

    #[test]
    fn labels_prefer_filesystem_label() {
        assert_eq!(drive_label("/", None), "Sistema (/)");
        assert_eq!(drive_label("/", Some("Raiz")), "Raiz (/)");
        assert_eq!(
            drive_label("/media/user/USB", None),
            "USB (/media/user/USB)"
        );
        assert_eq!(unescape_udev_label("Meu\\x20Disco"), "Meu Disco");
    }

    /// Coleta real desta máquina: a raiz aparece com capacidade coerente.
    #[test]
    fn real_system_drives_include_root_with_capacity() {
        let drives = system_drives();
        let root = drives.iter().find(|d| d.path == "/").expect("raiz montada");
        let (total, free) = (root.total_bytes.unwrap(), root.free_bytes.unwrap());
        assert!(total > 0 && free <= total, "{root:?}");
        assert!(drives
            .iter()
            .all(|d| !d.path.starts_with("/proc") && !d.path.starts_with("/var/lib/docker")));
    }
}
