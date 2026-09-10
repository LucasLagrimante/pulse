import os
import sqlite3
import subprocess
import sys
import time
from pathlib import Path

from textual.app import App, ComposeResult
from textual.binding import Binding
from textual.containers import Horizontal
from textual.widgets import DataTable, Footer, Header, Static
from textual_image.widget import AutoImage, BaseImage, HalfcellImage, SixelImage, TGPImage, UnicodeImage

from categories import categorize
from storage import query_recent

CATEGORY_LABELS = {
    "code": "Código",
    "image": "Imagem",
    "media": "Mídia",
    "document": "Documento",
    "installer": "Instalador",
}

# A detecção automática (AutoImage) escolhe o protocolo gráfico uma única vez,
# na inicialização, e é sabidamente instável (a própria lib avisa que leituras
# concorrentes do terminal podem dar falso positivo). "preview_protocol" no
# config.toml permite forçar um protocolo específico quando a detecção erra.
PREVIEW_WIDGET_CLASSES = {
    "auto": AutoImage,
    "sixel": SixelImage,
    "tgp": TGPImage,
    "halfcell": HalfcellImage,
    "unicode": UnicodeImage,
}


def resolve_preview_widget_class(protocol: str) -> type[BaseImage]:
    return PREVIEW_WIDGET_CLASSES.get(protocol, AutoImage)


def relative_time(seconds_ago: float) -> str:
    seconds_ago = max(0, seconds_ago)
    if seconds_ago < 60:
        return "agora mesmo"
    minutes = seconds_ago / 60
    if minutes < 60:
        return f"{int(minutes)} min atrás"
    hours = minutes / 60
    if hours < 24:
        return f"{int(hours)}h atrás"
    days = hours / 24
    return f"{int(days)}d atrás"


def default_open_folder(folder: str) -> None:
    if sys.platform == "win32":
        os.startfile(folder)  # type: ignore[attr-defined]
    else:
        subprocess.run(["xdg-open", folder], check=False)


class RecentFilesApp(App):
    # As bindings de categoria ficam com show=False: os nomes reais das
    # categorias já aparecem em #category-bar, e o rodapé só tem espaço para
    # descrições genéricas ("Categoria 1") que não dizem qual é qual.
    BINDINGS = [
        ("o", "open_selected", "Abrir pasta"),
        Binding("1", "toggle_category(0)", "Categoria 1", show=False),
        Binding("2", "toggle_category(1)", "Categoria 2", show=False),
        Binding("3", "toggle_category(2)", "Categoria 3", show=False),
        Binding("4", "toggle_category(3)", "Categoria 4", show=False),
        Binding("5", "toggle_category(4)", "Categoria 5", show=False),
        ("q", "quit", "Sair"),
    ]

    def __init__(
        self,
        conn: sqlite3.Connection,
        all_categories: list[str],
        open_folder=default_open_folder,
        event_queue=None,
        queue_poll_interval: float = 1.0,
        preview_protocol: str = "auto",
    ):
        super().__init__()
        self._conn = conn
        self._all_categories = all_categories
        self._enabled_categories = set(all_categories)
        self._open_folder = open_folder
        self._search = ""
        self._event_queue = event_queue
        self._queue_poll_interval = queue_poll_interval
        self._preview_widget_class = resolve_preview_widget_class(preview_protocol)

    CSS = """
    #preview {
        width: 45%;
        border-left: solid $panel;
    }
    """

    def compose(self) -> ComposeResult:
        yield Header()
        yield Static(id="category-bar")
        with Horizontal():
            yield DataTable(id="files-table")
            yield self._preview_widget_class(id="preview")
        yield Footer()

    def on_mount(self) -> None:
        table = self.query_one("#files-table", DataTable)
        table.cursor_type = "row"
        table.add_columns("Nome", "Categoria", "Quando", "Pasta")
        self.query_one("#preview", BaseImage).display = False
        self.refresh_table()
        if self._event_queue is not None:
            self.set_interval(self._queue_poll_interval, self._drain_event_queue)

    def on_data_table_row_highlighted(self, event: DataTable.RowHighlighted) -> None:
        self._update_preview(event.row_key.value if event.row_key is not None else None)

    def _update_preview(self, path_str: str | None) -> None:
        preview = self.query_one("#preview", BaseImage)
        if path_str and categorize(Path(path_str)) == "image" and Path(path_str).is_file():
            preview.image = path_str
            preview.display = True
        else:
            preview.image = None
            preview.display = False

    def _sync_preview_with_cursor(self) -> None:
        table = self.query_one("#files-table", DataTable)
        if table.row_count == 0 or table.cursor_row is None:
            self._update_preview(None)
            return
        row_key, _ = table.coordinate_to_cell_key(table.cursor_coordinate)
        self._update_preview(row_key.value)

    def _category_bar_text(self) -> str:
        # Colchetes colidem com a sintaxe de markup do Rich (usada pelo Static
        # do Textual) e são silenciosamente descartados — por isso usamos
        # parênteses e um marcador ✓/· em vez de "[x]"/"[ ]".
        parts = []
        for i, category in enumerate(self._all_categories, start=1):
            label = CATEGORY_LABELS.get(category, category)
            mark = "✓" if category in self._enabled_categories else "·"
            parts.append(f"({i}) {mark} {label}")
        return "   ".join(parts)

    def _drain_event_queue(self) -> None:
        import queue as queue_module

        while True:
            try:
                item = self._event_queue.get_nowait()
            except queue_module.Empty:
                break
            self.feed_event(item)

    def refresh_table(self) -> None:
        self.query_one("#category-bar", Static).update(self._category_bar_text())
        table = self.query_one("#files-table", DataTable)
        table.clear()
        now = time.time()
        rows = query_recent(self._conn, categories=sorted(self._enabled_categories), search=self._search or None)
        for row in rows:
            path = Path(row["path"])
            table.add_row(
                path.name,
                CATEGORY_LABELS.get(row["category"], row["category"]),
                relative_time(now - row["seen_at"]),
                str(path.parent),
                key=row["path"],
            )
        self._sync_preview_with_cursor()

    def feed_event(self, item: dict) -> None:
        from storage import upsert_file

        upsert_file(
            self._conn,
            path=item["path"],
            category=item["category"],
            size=item["size"],
            created_at=item["created_at"],
            seen_at=time.time(),
        )
        self.refresh_table()

    def action_toggle_category(self, index: int) -> None:
        if index >= len(self._all_categories):
            return
        category = self._all_categories[index]
        if category in self._enabled_categories:
            self._enabled_categories.discard(category)
        else:
            self._enabled_categories.add(category)
        self.refresh_table()

    def action_open_selected(self) -> None:
        table = self.query_one("#files-table", DataTable)
        if table.cursor_row is None or table.row_count == 0:
            return
        row_key, _ = table.coordinate_to_cell_key(table.cursor_coordinate)
        self._open_path(row_key.value)

    def on_data_table_row_selected(self, event: DataTable.RowSelected) -> None:
        self._open_path(event.row_key.value)

    def _open_path(self, path: str) -> None:
        folder = str(Path(path).parent)
        self._open_folder(folder)
