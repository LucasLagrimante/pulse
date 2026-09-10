import sqlite3
from pathlib import Path

_SCHEMA = """
CREATE TABLE IF NOT EXISTS files (
    path TEXT PRIMARY KEY,
    category TEXT NOT NULL,
    size INTEGER NOT NULL,
    created_at REAL NOT NULL,
    seen_at REAL NOT NULL
)
"""


def connect(db_path: Path) -> sqlite3.Connection:
    db_path.parent.mkdir(parents=True, exist_ok=True)
    conn = sqlite3.connect(db_path)
    conn.row_factory = sqlite3.Row
    conn.execute(_SCHEMA)
    conn.commit()
    return conn


def upsert_file(conn: sqlite3.Connection, *, path: str, category: str, size: int, created_at: float, seen_at: float) -> None:
    conn.execute(
        """
        INSERT INTO files (path, category, size, created_at, seen_at)
        VALUES (:path, :category, :size, :created_at, :seen_at)
        ON CONFLICT(path) DO UPDATE SET
            category = excluded.category,
            size = excluded.size,
            seen_at = excluded.seen_at
        """,
        {"path": path, "category": category, "size": size, "created_at": created_at, "seen_at": seen_at},
    )
    conn.commit()


def query_recent(conn: sqlite3.Connection, categories: list[str] | None = None, search: str | None = None, limit: int = 200) -> list[sqlite3.Row]:
    query = "SELECT * FROM files WHERE 1=1"
    params: list = []

    if categories:
        placeholders = ", ".join("?" for _ in categories)
        query += f" AND category IN ({placeholders})"
        params.extend(categories)

    if search:
        query += " AND path LIKE ? ESCAPE '\\'"
        escaped = search.replace("\\", "\\\\").replace("%", "\\%").replace("_", "\\_")
        params.append(f"%{escaped}%")

    query += " ORDER BY seen_at DESC LIMIT ?"
    params.append(limit)

    return conn.execute(query, params).fetchall()
