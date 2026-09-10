import os
import time

from scanner import scan_folder

DAY = 86400


def _touch(path, mtime):
    path.write_text("x")
    os.utime(path, (mtime, mtime))


def test_scan_folder_finds_matching_recent_file(tmp_path):
    now = time.time()
    _touch(tmp_path / "script.py", now)

    results = scan_folder(tmp_path, enabled_categories=["code"], max_age_days=30, now=now)

    assert len(results) == 1
    assert results[0]["path"] == str(tmp_path / "script.py")
    assert results[0]["category"] == "code"


def test_scan_folder_skips_disabled_category(tmp_path):
    now = time.time()
    _touch(tmp_path / "photo.png", now)

    results = scan_folder(tmp_path, enabled_categories=["code"], max_age_days=30, now=now)

    assert results == []


def test_scan_folder_skips_unrecognized_extension(tmp_path):
    now = time.time()
    _touch(tmp_path / "data.bin", now)

    results = scan_folder(tmp_path, enabled_categories=["code", "image", "media", "document"], max_age_days=30, now=now)

    assert results == []


def test_scan_folder_skips_files_older_than_max_age(tmp_path):
    now = time.time()
    _touch(tmp_path / "old.py", now - 60 * DAY)

    results = scan_folder(tmp_path, enabled_categories=["code"], max_age_days=30, now=now)

    assert results == []


def test_scan_folder_skips_ignored_directories(tmp_path):
    now = time.time()
    nested = tmp_path / "node_modules" / "pkg"
    nested.mkdir(parents=True)
    _touch(nested / "index.js", now)

    results = scan_folder(tmp_path, enabled_categories=["code"], max_age_days=30, now=now)

    assert results == []
