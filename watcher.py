import logging
import queue
import time
from pathlib import Path

from watchdog.events import FileSystemEventHandler
from watchdog.observers import Observer

from categories import categorize, is_ignored

logger = logging.getLogger(__name__)


class _QueueEventHandler(FileSystemEventHandler):
    def __init__(self, event_queue: queue.Queue, enabled_categories: list[str]):
        self._queue = event_queue
        self._enabled_categories = set(enabled_categories)

    def on_created(self, event):
        if not event.is_directory:
            self._handle(Path(event.src_path))

    def on_moved(self, event):
        if not event.is_directory:
            self._handle(Path(event.dest_path))

    def _handle(self, path: Path):
        if is_ignored(path):
            return
        category = categorize(path)
        if category is None or category not in self._enabled_categories:
            return
        try:
            size = path.stat().st_size
        except OSError:
            return
        self._queue.put({
            "path": str(path),
            "category": category,
            "size": size,
            "created_at": time.time(),
        })


def start_watchers(folders: list[Path], enabled_categories: list[str], event_queue: queue.Queue) -> list[Observer]:
    observers = []
    handler = _QueueEventHandler(event_queue, enabled_categories)

    for folder in folders:
        if not folder.is_dir():
            logger.warning("Pasta monitorada não existe, pulando: %s", folder)
            continue
        try:
            observer = Observer()
            observer.schedule(handler, str(folder), recursive=True)
            observer.start()
            observers.append(observer)
        except OSError as exc:
            logger.warning("Não foi possível monitorar %s: %s", folder, exc)

    return observers


def stop_watchers(observers: list[Observer]) -> None:
    for observer in observers:
        observer.stop()
    for observer in observers:
        observer.join()
