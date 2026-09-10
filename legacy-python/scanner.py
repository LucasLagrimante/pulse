import os
from pathlib import Path

from categories import IGNORED_DIR_NAMES, categorize

DAY = 86400


def scan_folder(folder: Path, enabled_categories: list[str], max_age_days: int, now: float) -> list[dict]:
    min_mtime = now - max_age_days * DAY
    results = []

    for dirpath, dirnames, filenames in os.walk(folder):
        dirnames[:] = [d for d in dirnames if d not in IGNORED_DIR_NAMES]

        for filename in filenames:
            path = Path(dirpath) / filename
            category = categorize(path)
            if category is None or category not in enabled_categories:
                continue

            try:
                stat = path.stat()
            except OSError:
                continue

            if stat.st_mtime < min_mtime:
                continue

            results.append({
                "path": str(path),
                "category": category,
                "size": stat.st_size,
                "created_at": stat.st_mtime,
            })

    return results
