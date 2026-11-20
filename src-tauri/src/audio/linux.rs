//! Backend de áudio do Linux via `pactl` (PulseAudio ou PipeWire com
//! `pipewire-pulse`).
//!
//! Usa a saída em texto com `LC_ALL=C`: o `--format=json` do pactl 16 troca
//! por `(null)` qualquer descrição com caractere não ASCII (ex.: "Estéreo
//! analógico"), então o texto é a fonte mais fiel e funciona em qualquer versão.
//! Identificador dos dispositivos = `name` do PulseAudio (estável entre boots).

use super::{AudioDevice, AudioDeviceInventory, AudioMixerApp, AudioMixerInventory};
use std::collections::HashMap;
use std::io::Read;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

const PACTL_TIMEOUT: Duration = Duration::from_secs(4);
/// Volume nominal (100%) do PulseAudio.
const PA_VOLUME_NORM: f64 = 65536.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Kind {
    Sink,
    Source,
    SinkInput,
}

/// Um objeto da saída de `pactl list` (sink, source ou sink-input).
#[derive(Debug, Clone, Default, PartialEq)]
pub(super) struct PaObject {
    pub index: u32,
    pub name: String,
    pub description: String,
    /// Sink-input: índice do sink onde toca.
    pub sink: Option<u32>,
    pub mute: bool,
    /// Média dos canais em escala linear (1.0 = 100%; pode passar de 1.0).
    pub volume: Option<f64>,
    /// Source: nome do sink monitorado (`None` = microfone/entrada real).
    pub monitor_of_sink: Option<String>,
    pub properties: HashMap<String, String>,
}

impl PaObject {
    fn prop(&self, key: &str) -> Option<&str> {
        self.properties
            .get(key)
            .map(String::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty() && *value != "(null)")
    }

    fn display_name(&self) -> String {
        [
            Some(self.description.trim()).filter(|d| !d.is_empty() && *d != "(null)"),
            self.prop("device.description"),
            self.prop("node.nick"),
        ]
        .into_iter()
        .flatten()
        .next()
        .unwrap_or(self.name.as_str())
        .to_owned()
    }

    fn is_monitor(&self) -> bool {
        self.monitor_of_sink.is_some() || self.prop("device.class") == Some("monitor")
    }
}

fn header_kind(line: &str) -> Option<(Kind, u32)> {
    let (label, index) = line.trim_end().split_once(" #")?;
    let kind = match label {
        "Sink" => Kind::Sink,
        "Source" => Kind::Source,
        "Sink Input" => Kind::SinkInput,
        _ => return None,
    };
    Some((kind, index.trim().parse().ok()?))
}

/// `front-left: 65536 / 100% / 0.00 dB,   front-right: 32768 /  50% / ...`
/// → média dos valores brutos / 65536.
pub(super) fn parse_volume(value: &str) -> Option<f64> {
    let raw: Vec<f64> = value
        .split(',')
        .filter_map(|channel| {
            let (_, rest) = channel.split_once(':')?;
            rest.split('/').next()?.trim().parse::<u32>().ok()
        })
        .map(f64::from)
        .collect();
    if raw.is_empty() {
        return None;
    }
    Some(raw.iter().sum::<f64>() / raw.len() as f64 / PA_VOLUME_NORM)
}

fn unquote(value: &str) -> String {
    let value = value.trim();
    let inner = value
        .strip_prefix('"')
        .and_then(|v| v.strip_suffix('"'))
        .unwrap_or(value);
    inner.replace("\\\"", "\"")
}

/// Interpreta a saída de `LC_ALL=C pactl list sinks|sources|sink-inputs`.
pub(super) fn parse_list(text: &str, kind: Kind) -> Vec<PaObject> {
    let mut objects = Vec::new();
    let mut current: Option<PaObject> = None;
    let mut section = String::new();

    for line in text.lines() {
        if !line.starts_with('\t') && !line.starts_with(' ') {
            if let Some(object) = current.take() {
                objects.push(object);
            }
            section.clear();
            if let Some((found, index)) = header_kind(line) {
                if found == kind {
                    current = Some(PaObject {
                        index,
                        ..Default::default()
                    });
                }
            }
            continue;
        }
        let Some(object) = current.as_mut() else {
            continue;
        };
        if let Some(field) = line.strip_prefix('\t').filter(|l| !l.starts_with('\t')) {
            let Some((key, value)) = field.split_once(':') else {
                continue;
            };
            let value = value.trim();
            section = if value.is_empty() {
                key.to_owned()
            } else {
                String::new()
            };
            match key {
                "Name" => object.name = value.to_owned(),
                "Description" => object.description = value.to_owned(),
                "Mute" => object.mute = value == "yes",
                "Volume" => object.volume = parse_volume(value),
                "Sink" if kind == Kind::SinkInput => object.sink = value.parse().ok(),
                "Monitor of Sink" => {
                    object.monitor_of_sink =
                        (value != "n/a" && !value.is_empty()).then(|| value.to_owned());
                }
                _ => {}
            }
        } else if section == "Properties" {
            if let Some((key, value)) = line.trim().split_once(" = ") {
                object
                    .properties
                    .insert(key.trim().to_owned(), unquote(value));
            }
        }
    }
    if let Some(object) = current.take() {
        objects.push(object);
    }
    objects
}

/// `Default Sink: x` / `Default Source: y` de `pactl info`.
pub(super) fn parse_defaults(info: &str) -> (Option<String>, Option<String>) {
    let mut sink = None;
    let mut source = None;
    for line in info.lines() {
        if let Some(value) = line.strip_prefix("Default Sink:") {
            sink = Some(value.trim().to_owned()).filter(|v| !v.is_empty());
        } else if let Some(value) = line.strip_prefix("Default Source:") {
            source = Some(value.trim().to_owned()).filter(|v| !v.is_empty());
        }
    }
    (sink, source)
}

/// Executa `pactl` com locale C e prazo fixo. Argumentos sempre vêm de
/// constantes ou de nomes/índices já confirmados na listagem do próprio pactl.
fn pactl(args: &[&str]) -> Result<String, String> {
    let mut child = Command::new("pactl")
        .args(args)
        .env("LC_ALL", "C")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                "O utilitário pactl não está instalado (pacote pulseaudio-utils ou pipewire-pulse)."
                    .to_owned()
            } else {
                format!("Não foi possível executar o pactl: {error}")
            }
        })?;

    let mut stdout = child.stdout.take();
    let mut stderr = child.stderr.take();
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        let mut out = Vec::new();
        let mut err = Vec::new();
        if let Some(pipe) = stdout.as_mut() {
            let _ = pipe.read_to_end(&mut out);
        }
        if let Some(pipe) = stderr.as_mut() {
            let _ = pipe.read_to_end(&mut err);
        }
        let _ = sender.send((out, err));
    });

    let deadline = Instant::now() + PACTL_TIMEOUT;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(15)),
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err("O servidor de áudio não respondeu ao pactl dentro de 4 s.".to_owned());
            }
            Err(error) => return Err(format!("Falha ao aguardar o pactl: {error}")),
        }
    };
    let (out, err) = receiver
        .recv_timeout(Duration::from_secs(1))
        .unwrap_or_default();
    if !status.success() {
        let detail = String::from_utf8_lossy(&err).trim().to_owned();
        return Err(if detail.is_empty() {
            "O pactl não conseguiu falar com o servidor de áudio (PulseAudio/PipeWire).".to_owned()
        } else {
            format!("O servidor de áudio recusou o comando: {detail}")
        });
    }
    Ok(String::from_utf8_lossy(&out).into_owned())
}

fn list_objects(kind: Kind) -> Result<Vec<PaObject>, String> {
    let what = match kind {
        Kind::Sink => "sinks",
        Kind::Source => "sources",
        Kind::SinkInput => "sink-inputs",
    };
    pactl(&["list", what]).map(|text| parse_list(&text, kind))
}

fn record_failure(operation: &str, started: Instant, stage: &str, detail: &str) {
    crate::debug::record_failure(
        operation,
        started.elapsed().as_millis(),
        crate::debug::Failure::new("Falha ao consultar o áudio pelo pactl")
            .stage(stage)
            .detail(detail),
    );
}

struct Snapshot {
    sinks: Vec<PaObject>,
    sources: Vec<PaObject>,
    default_sink: Option<String>,
    default_source: Option<String>,
}

fn snapshot() -> Result<Snapshot, String> {
    let (default_sink, default_source) = parse_defaults(&pactl(&["info"])?);
    Ok(Snapshot {
        sinks: list_objects(Kind::Sink)?,
        sources: list_objects(Kind::Source)?,
        default_sink,
        default_source,
    })
}

fn device(object: &PaObject, default: Option<&str>) -> AudioDevice {
    AudioDevice {
        id: object.name.clone(),
        name: object.display_name(),
        is_default: default == Some(object.name.as_str()),
    }
}

fn inventory_from(snapshot: &Snapshot) -> AudioDeviceInventory {
    let output: Vec<AudioDevice> = snapshot
        .sinks
        .iter()
        .filter(|sink| !sink.name.is_empty())
        .map(|sink| device(sink, snapshot.default_sink.as_deref()))
        .collect();
    let input: Vec<AudioDevice> = snapshot
        .sources
        .iter()
        .filter(|source| !source.name.is_empty() && !source.is_monitor())
        .map(|source| device(source, snapshot.default_source.as_deref()))
        .collect();
    AudioDeviceInventory {
        output_default_id: output.iter().find(|d| d.is_default).map(|d| d.id.clone()),
        input_default_id: input.iter().find(|d| d.is_default).map(|d| d.id.clone()),
        input,
        output,
        unavailable_reason: None,
    }
}

pub fn list() -> AudioDeviceInventory {
    let started = Instant::now();
    match snapshot() {
        Ok(snapshot) => inventory_from(&snapshot),
        Err(reason) => {
            record_failure("audio_devices", started, "pactl list", &reason);
            AudioDeviceInventory {
                unavailable_reason: Some(reason),
                ..Default::default()
            }
        }
    }
}

pub fn set_default(direction: &str, device_id: &str) -> Result<AudioDeviceInventory, String> {
    let current = snapshot()?;
    let inventory = inventory_from(&current);
    let (devices, command) = match direction {
        "output" => (&inventory.output, "set-default-sink"),
        "input" => (&inventory.input, "set-default-source"),
        _ => return Err(format!("Direção de áudio inválida: {direction}.")),
    };
    // Só aceita um nome que o próprio servidor de áudio listou agora.
    let target = devices
        .iter()
        .find(|device| device.id == device_id)
        .ok_or_else(|| "O dispositivo de áudio escolhido não está mais disponível.".to_owned())?;
    pactl(&[command, &target.id])?;
    Ok(list())
}

/// Chave de agrupamento por app (igual ao mixer do Windows, que agrupa por
/// executável): binário do processo, senão nome do app, senão o próprio stream.
fn app_key(stream: &PaObject) -> (String, String, bool) {
    if stream.prop("media.role") == Some("event") {
        return ("system".to_owned(), "Sons do sistema".to_owned(), true);
    }
    let binary = stream.prop("application.process.binary");
    let app_name = stream.prop("application.name");
    let key = binary
        .or(app_name)
        .map(|value| value.to_lowercase())
        .unwrap_or_else(|| format!("stream-{}", stream.index));
    let display = app_name
        .or(binary)
        .or_else(|| stream.prop("media.name"))
        .map(str::to_owned)
        .unwrap_or_else(|| format!("Fluxo de áudio {}", stream.index));
    (key, display, false)
}

fn clamp_level(value: f64) -> f32 {
    if value.is_finite() {
        value.clamp(0.0, 1.0) as f32
    } else {
        0.0
    }
}

pub(super) fn group_streams(
    streams: &[PaObject],
    sink_index: u32,
) -> Vec<(AudioMixerApp, Vec<u32>)> {
    let mut order: Vec<String> = Vec::new();
    let mut groups: HashMap<String, (AudioMixerApp, Vec<u32>, Vec<f64>)> = HashMap::new();
    for stream in streams.iter().filter(|s| s.sink == Some(sink_index)) {
        let (key, display, is_system) = app_key(stream);
        let entry = groups.entry(key.clone()).or_insert_with(|| {
            order.push(key.clone());
            (
                AudioMixerApp {
                    app_name: key.clone(),
                    display_name: display,
                    process_ids: Vec::new(),
                    session_count: 0,
                    volume: 0.0,
                    muted: true,
                    is_system_sounds: is_system,
                    icon: None,
                },
                Vec::new(),
                Vec::new(),
            )
        });
        entry.0.session_count += 1;
        entry.0.muted &= stream.mute;
        if let Some(pid) = stream
            .prop("application.process.id")
            .and_then(|pid| pid.parse::<u32>().ok())
        {
            if !entry.0.process_ids.contains(&pid) {
                entry.0.process_ids.push(pid);
            }
        }
        entry.1.push(stream.index);
        if let Some(volume) = stream.volume {
            entry.2.push(volume);
        }
    }
    order
        .into_iter()
        .filter_map(|key| groups.remove(&key))
        // Sem leitura de volume não há o que mostrar (nunca um 0 fictício).
        .filter(|(_, _, volumes)| !volumes.is_empty())
        .map(|(mut app, indices, volumes)| {
            app.volume = clamp_level(volumes.iter().sum::<f64>() / volumes.len() as f64);
            (app, indices)
        })
        .collect()
}

struct MixerTarget {
    sink: PaObject,
    streams: Vec<(AudioMixerApp, Vec<u32>)>,
}

fn resolve_mixer(device_id: Option<&str>) -> Result<MixerTarget, String> {
    let (default_sink, _) = parse_defaults(&pactl(&["info"])?);
    let sinks = list_objects(Kind::Sink)?;
    let wanted = device_id
        .filter(|id| !id.trim().is_empty())
        .map(str::to_owned)
        .or(default_sink)
        .ok_or_else(|| "Nenhuma saída de áudio padrão definida.".to_owned())?;
    let sink = sinks
        .into_iter()
        .find(|sink| sink.name == wanted)
        .ok_or_else(|| "A saída de áudio escolhida não está mais disponível.".to_owned())?;
    let streams = group_streams(&list_objects(Kind::SinkInput)?, sink.index);
    Ok(MixerTarget { sink, streams })
}

pub fn mixer(device_id: Option<&str>) -> AudioMixerInventory {
    let started = Instant::now();
    match resolve_mixer(device_id) {
        Ok(target) => AudioMixerInventory {
            device_id: Some(target.sink.name.clone()),
            device_name: Some(target.sink.display_name()),
            master_volume: target.sink.volume.map(clamp_level),
            master_muted: Some(target.sink.mute),
            apps: target.streams.into_iter().map(|(app, _)| app).collect(),
            unavailable_reason: None,
        },
        Err(reason) => {
            record_failure("audio_mixer", started, "pactl list sink-inputs", &reason);
            AudioMixerInventory {
                unavailable_reason: Some(reason),
                ..Default::default()
            }
        }
    }
}

fn raw_volume(level: f32) -> Result<String, String> {
    if !level.is_finite() {
        return Err("Volume inválido.".to_owned());
    }
    let raw = (f64::from(level.clamp(0.0, 1.0)) * PA_VOLUME_NORM).round() as u32;
    Ok(raw.to_string())
}

fn app_streams(target: &MixerTarget, app_name: &str) -> Result<Vec<String>, String> {
    target
        .streams
        .iter()
        .find(|(app, _)| app.app_name == app_name)
        .map(|(_, indices)| indices.iter().map(u32::to_string).collect())
        .ok_or_else(|| format!("{app_name} não está mais tocando nesta saída de áudio."))
}

pub fn set_mixer_app_volume(
    device_id: Option<&str>,
    app_name: &str,
    volume: f32,
) -> Result<AudioMixerInventory, String> {
    let raw = raw_volume(volume)?;
    let target = resolve_mixer(device_id)?;
    for index in app_streams(&target, app_name)? {
        pactl(&["set-sink-input-volume", &index, &raw])?;
    }
    Ok(mixer(Some(&target.sink.name)))
}

pub fn set_mixer_app_muted(
    device_id: Option<&str>,
    app_name: &str,
    muted: bool,
) -> Result<AudioMixerInventory, String> {
    let target = resolve_mixer(device_id)?;
    let flag = if muted { "1" } else { "0" };
    for index in app_streams(&target, app_name)? {
        pactl(&["set-sink-input-mute", &index, flag])?;
    }
    Ok(mixer(Some(&target.sink.name)))
}

pub fn set_mixer_master(
    device_id: Option<&str>,
    volume: Option<f32>,
    muted: Option<bool>,
) -> Result<AudioMixerInventory, String> {
    let target = resolve_mixer(device_id)?;
    let name = target.sink.name.clone();
    if let Some(level) = volume {
        pactl(&["set-sink-volume", &name, &raw_volume(level)?])?;
    }
    if let Some(mute) = muted {
        pactl(&["set-sink-mute", &name, if mute { "1" } else { "0" }])?;
    }
    Ok(mixer(Some(&name)))
}

#[cfg(test)]
mod tests {
    use super::*;

    const SINKS: &str = "Sink #56\n\tState: SUSPENDED\n\tName: alsa_output.hdmi\n\tDescription: HDA NVidia Estéreo digital (HDMI)\n\tDriver: PipeWire\n\tMute: no\n\tVolume: front-left: 65536 / 100% / 0.00 dB,   front-right: 32768 /  50% / -18.06 dB\n\t        balance 0.00\n\tProperties:\n\t\tdevice.description = \"HDA NVidia\"\n\t\tmedia.class = \"Audio/Sink\"\n\tPorts:\n\t\t[Out] HDMI1: HDMI (type: HDMI, priority: 100, available)\n\tActive Port: [Out] HDMI1\n\nSink #84140\n\tName: alsa_output.usb\n\tDescription: (null)\n\tMute: yes\n\tVolume: mono: 98304 / 150% / 10.57 dB\n\tProperties:\n\t\tnode.nick = \"Headset \\\"USB\\\"\"\n";

    const SOURCES: &str = "Source #57\n\tName: alsa_output.hdmi.monitor\n\tDescription: Monitor of HDA\n\tMute: no\n\tVolume: front-left: 65536 / 100% / 0.00 dB\n\tMonitor of Sink: alsa_output.hdmi\n\tProperties:\n\t\tdevice.class = \"monitor\"\nSource #60\n\tName: alsa_input.mic\n\tDescription: Microfone\n\tMute: no\n\tVolume: mono: 65536 / 100% / 0.00 dB\n\tMonitor of Sink: n/a\n\tProperties:\n\t\tdevice.class = \"sound\"\n";

    const SINK_INPUTS: &str = "Sink Input #1\n\tSink: 56\n\tMute: no\n\tVolume: front-left: 32768 /  50% / -18.06 dB,   front-right: 32768 /  50% / -18.06 dB\n\tProperties:\n\t\tapplication.name = \"Firefox\"\n\t\tapplication.process.id = \"100\"\n\t\tapplication.process.binary = \"firefox\"\nSink Input #2\n\tSink: 56\n\tMute: yes\n\tVolume: front-left: 65536 / 100% / 0.00 dB\n\tProperties:\n\t\tapplication.name = \"Firefox\"\n\t\tapplication.process.id = \"101\"\n\t\tapplication.process.binary = \"firefox\"\nSink Input #3\n\tSink: 84140\n\tMute: no\n\tVolume: mono: 65536 / 100% / 0.00 dB\n\tProperties:\n\t\tapplication.name = \"Spotify\"\nSink Input #4\n\tSink: 56\n\tMute: no\n\tVolume: mono: 65536 / 100% / 0.00 dB\n\tProperties:\n\t\tmedia.role = \"event\"\n\t\tapplication.name = \"plasmashell\"\n";

    #[test]
    fn parses_sinks_with_utf8_descriptions_and_fallback_names() {
        let sinks = parse_list(SINKS, Kind::Sink);
        assert_eq!(sinks.len(), 2);
        assert_eq!(sinks[0].index, 56);
        assert_eq!(sinks[0].display_name(), "HDA NVidia Estéreo digital (HDMI)");
        assert!((sinks[0].volume.unwrap() - 0.75).abs() < 1e-9);
        assert!(!sinks[0].mute);
        // Seções "Ports" não viram propriedades.
        assert!(!sinks[0].properties.keys().any(|k| k.contains("HDMI1")));
        assert_eq!(sinks[1].display_name(), "Headset \"USB\"");
        assert!(sinks[1].mute);
        assert!((sinks[1].volume.unwrap() - 1.5).abs() < 1e-9);
    }

    #[test]
    fn skips_monitor_sources_from_inputs() {
        let sources = parse_list(SOURCES, Kind::Source);
        assert_eq!(sources.len(), 2);
        assert!(sources[0].is_monitor());
        assert!(!sources[1].is_monitor());
        let inventory = inventory_from(&Snapshot {
            sinks: parse_list(SINKS, Kind::Sink),
            sources,
            default_sink: Some("alsa_output.usb".into()),
            default_source: Some("alsa_input.mic".into()),
        });
        assert_eq!(inventory.input.len(), 1);
        assert_eq!(
            inventory.input_default_id.as_deref(),
            Some("alsa_input.mic")
        );
        assert_eq!(
            inventory.output_default_id.as_deref(),
            Some("alsa_output.usb")
        );
        assert_eq!(inventory.output.iter().filter(|d| d.is_default).count(), 1);
    }

    #[test]
    fn groups_streams_per_app_on_the_selected_sink() {
        let streams = parse_list(SINK_INPUTS, Kind::SinkInput);
        assert_eq!(streams.len(), 4);
        let groups = group_streams(&streams, 56);
        assert_eq!(groups.len(), 2);
        let (firefox, indices) = &groups[0];
        assert_eq!(firefox.app_name, "firefox");
        assert_eq!(firefox.display_name, "Firefox");
        assert_eq!(firefox.session_count, 2);
        assert_eq!(firefox.process_ids, vec![100, 101]);
        assert_eq!(indices, &vec![1, 2]);
        assert!(!firefox.muted, "só um dos fluxos está mudo");
        assert!((firefox.volume - 0.75).abs() < 1e-6);
        let (system, _) = &groups[1];
        assert!(system.is_system_sounds);
        assert_eq!(system.app_name, "system");
    }

    #[test]
    fn parses_defaults_and_volume_edge_cases() {
        let (sink, source) = parse_defaults(
            "Server Name: PulseAudio (on PipeWire 1.0.5)\nDefault Sink: a.b\nDefault Source: c.d\n",
        );
        assert_eq!(sink.as_deref(), Some("a.b"));
        assert_eq!(source.as_deref(), Some("c.d"));
        assert_eq!(parse_volume("sem canais"), None);
        assert_eq!(raw_volume(0.5).unwrap(), "32768");
        assert_eq!(raw_volume(2.0).unwrap(), "65536");
        assert!(raw_volume(f32::NAN).is_err());
    }

    /// Coleta real nesta máquina: só verifica forma e sanidade.
    #[test]
    fn live_pactl_inventory_is_sane_when_available() {
        let inventory = list();
        eprintln!(
            "pactl: {} saídas, {} entradas, padrão {:?} / indisponível: {:?}",
            inventory.output.len(),
            inventory.input.len(),
            inventory.output_default_id,
            inventory.unavailable_reason
        );
        if let Some(reason) = inventory.unavailable_reason.as_deref() {
            assert!(!reason.is_empty());
            assert!(inventory.output.is_empty() && inventory.input.is_empty());
            return;
        }
        assert!(inventory
            .output
            .iter()
            .all(|d| !d.id.is_empty() && !d.name.is_empty()));
        assert!(inventory.output.iter().filter(|d| d.is_default).count() <= 1);
        assert!(inventory.input.iter().all(|d| !d.id.ends_with(".monitor")));
        let mixer = mixer(None);
        eprintln!(
            "mixer: {:?} master {:?} apps {:?}",
            mixer.device_name,
            mixer.master_volume,
            mixer
                .apps
                .iter()
                .map(|a| (&a.app_name, a.volume, &a.process_ids))
                .collect::<Vec<_>>()
        );
        if mixer.unavailable_reason.is_none() {
            assert!(mixer.device_id.is_some());
            if let Some(volume) = mixer.master_volume {
                assert!((0.0..=1.0).contains(&volume));
            }
        }
    }
}
