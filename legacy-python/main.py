import logging
import queue
import time
from pathlib import Path

from app import RecentFilesApp
from config import load_config
from scanner import scan_folder
from storage import connect, upsert_file
from watcher import start_watchers, stop_watchers

APP_DIR = Path.home() / ".filewatch"
CONFIG_PATH = APP_DIR / "config.toml"
DB_PATH = APP_DIR / "history.db"
SCAN_MAX_AGE_DAYS = 30
QUEUE_POLL_INTERVAL_SECONDS = 1.0

logging.basicConfig(level=logging.WARNING)
logger = logging.getLogger(__name__)


def run_initial_scan(conn, folders: list[Path], enabled_categories: list[str]) -> None:
    now = time.time()
    for folder in folders:
        if not folder.is_dir():
            logger.warning("Pasta monitorada não existe, pulando varredura inicial: %s", folder)
            continue
        for entry in scan_folder(folder, enabled_categories, SCAN_MAX_AGE_DAYS, now):
            upsert_file(
                conn,
                path=entry["path"],
                category=entry["category"],
                size=entry["size"],
                created_at=entry["created_at"],
                seen_at=entry["created_at"],
            )


def main() -> None:
    config = load_config(CONFIG_PATH, Path.home())
    watched_folders = [Path(p) for p in config["watched_folders"]]
    all_categories = config["categories"]

    conn = connect(DB_PATH)
    run_initial_scan(conn, watched_folders, all_categories)

    event_queue: queue.Queue = queue.Queue()
    observers = start_watchers(watched_folders, all_categories, event_queue)

    app = RecentFilesApp(
        conn=conn,
        all_categories=all_categories,
        event_queue=event_queue,
        queue_poll_interval=QUEUE_POLL_INTERVAL_SECONDS,
        preview_protocol=config.get("preview_protocol", "auto"),
    )

    try:
        app.run()
    finally:
        stop_watchers(observers)
        conn.close()


if __name__ == "__main__":
    main()
