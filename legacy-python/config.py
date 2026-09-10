import string
import sys
import tomllib
from pathlib import Path

from categories import EXTENSIONS_BY_CATEGORY, IGNORED_DIR_NAMES

DEFAULT_FOLDER_NAMES = ["Downloads", "Desktop", "Documents", "Pictures", "Videos", "Music"]


def default_watched_folders(home: Path) -> list[Path]:
    return [home / name for name in DEFAULT_FOLDER_NAMES if (home / name).is_dir()]


def available_drive_roots() -> list[Path]:
    if sys.platform != "win32":
        return []
    return [
        root for letter in string.ascii_uppercase
        if (root := Path(f"{letter}:\\")).exists()
    ]


def other_drive_watched_folders(home: Path, drive_roots: list[Path], exists=None) -> list[Path]:
    if exists is None:
        exists = lambda p: p.is_dir()  # noqa: E731

    home_drive = home.drive.upper()
    folders = []
    for root in drive_roots:
        if root.drive.upper() == home_drive:
            continue
        folders.extend(root / name for name in DEFAULT_FOLDER_NAMES if exists(root / name))
    return folders


def default_config(home: Path, drive_roots: list[Path] | None = None) -> dict:
    if drive_roots is None:
        drive_roots = available_drive_roots()

    folders = default_watched_folders(home) + other_drive_watched_folders(home, drive_roots)
    return {
        "watched_folders": [str(p) for p in folders],
        "categories": sorted(EXTENSIONS_BY_CATEGORY.keys()),
        "ignore_dirs": sorted(IGNORED_DIR_NAMES),
        # "auto" detecta o protocolo gráfico do terminal (Sixel/Kitty) uma
        # única vez, na inicialização — mas a detecção é sabidamente instável
        # (a própria lib avisa: leituras concorrentes do terminal podem dar
        # falso positivo). Se as imagens saírem pretas/quebradas, force
        # "halfcell" ou "unicode" aqui — mais blocudo, porém confiável em
        # qualquer terminal.
        "preview_protocol": "auto",
    }


def _toml_string_array(values: list[str]) -> str:
    escaped = [v.replace("\\", "\\\\").replace('"', '\\"') for v in values]
    items = ", ".join(f'"{v}"' for v in escaped)
    return f"[{items}]"


def _toml_string(value: str) -> str:
    escaped = value.replace("\\", "\\\\").replace('"', '\\"')
    return f'"{escaped}"'


def save_config(config_path: Path, config: dict) -> None:
    lines = [
        f"watched_folders = {_toml_string_array(config['watched_folders'])}",
        f"categories = {_toml_string_array(config['categories'])}",
        f"ignore_dirs = {_toml_string_array(config['ignore_dirs'])}",
        f"preview_protocol = {_toml_string(config.get('preview_protocol', 'auto'))}",
    ]
    config_path.parent.mkdir(parents=True, exist_ok=True)
    config_path.write_text("\n".join(lines) + "\n", encoding="utf-8")


def load_config(config_path: Path, home: Path, drive_roots: list[Path] | None = None) -> dict:
    if not config_path.exists():
        config = default_config(home, drive_roots)
        save_config(config_path, config)
        return config

    with config_path.open("rb") as f:
        return tomllib.load(f)
