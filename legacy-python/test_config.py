from pathlib import Path

from config import (
    default_config,
    default_watched_folders,
    load_config,
    other_drive_watched_folders,
    save_config,
)


def test_default_watched_folders_only_includes_existing_dirs(tmp_path):
    (tmp_path / "Downloads").mkdir()
    (tmp_path / "Music").mkdir()

    folders = default_watched_folders(tmp_path)

    names = {f.name for f in folders}
    assert "Downloads" in names
    assert "Music" in names
    assert "Videos" not in names


def test_default_config_includes_all_categories(tmp_path):
    (tmp_path / "Downloads").mkdir()

    config = default_config(tmp_path, drive_roots=[])

    assert set(config["categories"]) == {"code", "image", "media", "document", "installer"}


def test_default_config_preview_protocol_defaults_to_auto(tmp_path):
    (tmp_path / "Downloads").mkdir()

    config = default_config(tmp_path, drive_roots=[])

    assert config["preview_protocol"] == "auto"


def test_save_then_load_round_trips_config(tmp_path):
    (tmp_path / "Downloads").mkdir()
    config_path = tmp_path / "config.toml"
    original = default_config(tmp_path, drive_roots=[])

    save_config(config_path, original)
    loaded = load_config(config_path, tmp_path, drive_roots=[])

    assert loaded["watched_folders"] == original["watched_folders"]
    assert loaded["categories"] == original["categories"]
    assert loaded["ignore_dirs"] == original["ignore_dirs"]
    assert loaded["preview_protocol"] == original["preview_protocol"]


def test_load_config_creates_default_when_missing(tmp_path):
    (tmp_path / "Downloads").mkdir()
    config_path = tmp_path / "config.toml"

    loaded = load_config(config_path, tmp_path, drive_roots=[])

    assert config_path.exists()
    assert loaded["categories"]


def test_other_drive_watched_folders_includes_existing_root_folders():
    home = Path("C:/Users/lucas")
    drive_d = Path("D:/")
    existing = {drive_d / "Downloads", drive_d / "Music"}

    folders = other_drive_watched_folders(home, drive_roots=[drive_d], exists=lambda p: p in existing)

    names = {f.name for f in folders}
    assert "Downloads" in names
    assert "Music" in names
    assert "Videos" not in names


def test_other_drive_watched_folders_skips_home_drive():
    home = Path("C:/Users/lucas")
    same_drive_root = Path("C:/")

    folders = other_drive_watched_folders(home, drive_roots=[same_drive_root], exists=lambda p: True)

    assert folders == []
