import os
import time

from main import run_initial_scan
from storage import connect, query_recent


def test_run_initial_scan_populates_storage(tmp_path):
    folder = tmp_path / "Downloads"
    folder.mkdir()
    now = time.time()
    (folder / "script.py").write_text("x")
    os.utime(folder / "script.py", (now, now))

    conn = connect(tmp_path / "history.db")
    run_initial_scan(conn, [folder], enabled_categories=["code"])

    rows = query_recent(conn)
    assert len(rows) == 1
    assert rows[0]["path"].endswith("script.py")


def test_run_initial_scan_skips_missing_folder(tmp_path):
    conn = connect(tmp_path / "history.db")
    run_initial_scan(conn, [tmp_path / "does-not-exist"], enabled_categories=["code"])

    assert query_recent(conn) == []
