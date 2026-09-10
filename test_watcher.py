import queue
import shutil
import time
from pathlib import Path

import pytest

from watcher import start_watchers, stop_watchers


def _drain_until(q, predicate, timeout=3.0):
    deadline = time.time() + timeout
    while time.time() < deadline:
        try:
            item = q.get(timeout=0.2)
        except queue.Empty:
            continue
        if predicate(item):
            return item
    return None


@pytest.fixture
def watched_dir():
    # Fica fora de AppData\Local\Temp (que a ignore-list já filtra de propósito),
    # senão não conseguimos distinguir "diretório real ignorado" de "diretório de teste".
    d = Path(__file__).parent / "_test_scratch"
    d.mkdir(exist_ok=True)
    yield d
    shutil.rmtree(d, ignore_errors=True)


def test_watcher_reports_new_matching_file(watched_dir):
    q = queue.Queue()
    observers = start_watchers([watched_dir], enabled_categories=["code"], event_queue=q)
    try:
        (watched_dir / "novo.py").write_text("print(1)")
        item = _drain_until(q, lambda i: i["path"].endswith("novo.py"))
        assert item is not None
        assert item["category"] == "code"
    finally:
        stop_watchers(observers)


def test_watcher_ignores_non_matching_extension(watched_dir):
    q = queue.Queue()
    observers = start_watchers([watched_dir], enabled_categories=["code"], event_queue=q)
    try:
        (watched_dir / "dados.bin").write_text("x")
        item = _drain_until(q, lambda i: True, timeout=1.0)
        assert item is None
    finally:
        stop_watchers(observers)
