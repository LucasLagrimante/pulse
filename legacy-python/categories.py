from pathlib import Path

EXTENSIONS_BY_CATEGORY = {
    "code": {
        ".py", ".js", ".ts", ".jsx", ".tsx", ".sh", ".ps1", ".java", ".c",
        ".cpp", ".h", ".go", ".rs", ".json", ".yaml", ".yml", ".toml", ".sql",
    },
    "image": {".png", ".jpg", ".jpeg", ".gif", ".bmp", ".webp"},
    "media": {
        ".mp4", ".mov", ".mkv", ".avi", ".webm",
        ".mp3", ".wav", ".flac", ".m4a", ".ogg",
    },
    "document": {".pdf", ".docx", ".xlsx", ".pptx", ".txt", ".md"},
    "installer": {".exe", ".msi", ".zip", ".rar", ".7z", ".iso"},
}

_EXTENSION_TO_CATEGORY = {
    ext: category
    for category, extensions in EXTENSIONS_BY_CATEGORY.items()
    for ext in extensions
}

IGNORED_DIR_NAMES = {
    "node_modules", ".git", "__pycache__", "venv", ".venv", "dist", "build",
    "$RECYCLE.BIN", ".cache", "AppData",
}


def categorize(path: Path) -> str | None:
    return _EXTENSION_TO_CATEGORY.get(path.suffix.lower())


def is_ignored(path: Path) -> bool:
    return any(part in IGNORED_DIR_NAMES for part in path.parts)
