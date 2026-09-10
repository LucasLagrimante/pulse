import time
from pathlib import Path

import pytest
from PIL import Image as PILImage

from app import RecentFilesApp, relative_time
from storage import connect, upsert_file


def test_relative_time_seconds():
    assert relative_time(30) == "agora mesmo"


def test_relative_time_minutes():
    assert relative_time(5 * 60) == "5 min atrás"


def test_relative_time_hours():
    assert relative_time(3 * 3600) == "3h atrás"


def test_relative_time_days():
    assert relative_time(2 * 86400) == "2d atrás"


def _seed(conn, path, category, seen_at):
    upsert_file(conn, path=path, category=category, size=1, created_at=seen_at, seen_at=seen_at)


@pytest.mark.asyncio
async def test_toggling_category_off_hides_its_rows(tmp_path):
    conn = connect(tmp_path / "history.db")
    now = time.time()
    _seed(conn, "C:/Downloads/a.py", "code", now)
    _seed(conn, "C:/Downloads/b.png", "image", now)

    app = RecentFilesApp(conn=conn, all_categories=["code", "image"])
    async with app.run_test() as pilot:
        table = app.query_one("#files-table")
        assert table.row_count == 2

        await pilot.press("2")  # toggle off the 2nd category (image)

        assert table.row_count == 1


@pytest.mark.asyncio
async def test_category_bar_shows_names_and_toggle_state(tmp_path):
    conn = connect(tmp_path / "history.db")

    app = RecentFilesApp(conn=conn, all_categories=["code", "image"])
    async with app.run_test() as pilot:
        from textual.widgets import Static

        bar = app.query_one("#category-bar", Static)
        assert "Código" in str(bar.render())
        assert "Imagem" in str(bar.render())

        await pilot.press("2")  # desliga "image"

        bar_text = str(app.query_one("#category-bar", Static).render())
        assert "(1) ✓ Código" in bar_text
        assert "(2) · Imagem" in bar_text


@pytest.mark.asyncio
async def test_toggling_fifth_category_off_hides_its_rows(tmp_path):
    conn = connect(tmp_path / "history.db")
    now = time.time()
    # ordem alfabética das categorias: code, document, image, installer, media
    # -> a 5a tecla ("5") alterna a última, "media"
    _seed(conn, "D:/Downloads/setup.exe", "installer", now)
    _seed(conn, "D:/Music/song.mp3", "media", now)

    app = RecentFilesApp(conn=conn, all_categories=["code", "document", "image", "installer", "media"])
    async with app.run_test() as pilot:
        table = app.query_one("#files-table")
        assert table.row_count == 2

        await pilot.press("5")  # toggle off "media"

        assert table.row_count == 1


@pytest.mark.asyncio
async def test_category_toggle_bindings_are_hidden_from_footer(tmp_path):
    # A barra de categorias (#category-bar) já mostra os nomes reais; as
    # descrições genéricas "Categoria N" no rodapé (Footer) não ajudam e
    # confundem, então essas bindings devem ter show=False.
    conn = connect(tmp_path / "history.db")

    app = RecentFilesApp(conn=conn, all_categories=["code", "image"])
    async with app.run_test() as pilot:
        active = app.screen.active_bindings
        shown_descriptions = [
            binding.description for _, binding, _, _ in active.values() if binding.show
        ]
        assert not any("Categoria" in d for d in shown_descriptions)


@pytest.mark.asyncio
async def test_preview_protocol_can_be_forced_to_unicode(tmp_path):
    conn = connect(tmp_path / "history.db")

    app = RecentFilesApp(conn=conn, all_categories=["code", "image"], preview_protocol="unicode")
    async with app.run_test() as pilot:
        from textual_image.widget import UnicodeImage

        preview = app.query_one("#preview")
        assert isinstance(preview, UnicodeImage)


@pytest.mark.asyncio
async def test_preview_protocol_unknown_falls_back_to_auto(tmp_path):
    conn = connect(tmp_path / "history.db")

    app = RecentFilesApp(conn=conn, all_categories=["code", "image"], preview_protocol="nao-existe")
    async with app.run_test() as pilot:
        from textual_image.widget import AutoImage

        preview = app.query_one("#preview")
        assert isinstance(preview, AutoImage)


@pytest.mark.asyncio
async def test_preview_shows_for_image_row_and_hides_for_others(tmp_path):
    conn = connect(tmp_path / "history.db")
    now = time.time()
    img_path = tmp_path / "photo.png"
    PILImage.new("RGB", (4, 4), color="red").save(img_path)
    _seed(conn, str(img_path), "image", now)
    _seed(conn, "C:/Downloads/a.py", "code", now - 10)  # mais antigo, fica embaixo

    app = RecentFilesApp(conn=conn, all_categories=["code", "image"])
    async with app.run_test() as pilot:
        from textual_image.widget import AutoImage

        preview = app.query_one("#preview", AutoImage)
        assert preview.image is not None

        await pilot.press("down")  # move para a linha de código

        assert preview.image is None


@pytest.mark.asyncio
async def test_enter_opens_folder_of_selected_row(tmp_path):
    conn = connect(tmp_path / "history.db")
    now = time.time()
    _seed(conn, "C:/Downloads/a.py", "code", now)

    opened = []
    app = RecentFilesApp(conn=conn, all_categories=["code"], open_folder=lambda folder: opened.append(folder))
    async with app.run_test() as pilot:
        await pilot.press("enter")

    assert opened == [str(Path("C:/Downloads"))]
