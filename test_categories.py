from pathlib import Path

from categories import categorize, is_ignored


def test_categorize_python_file_is_code():
    assert categorize(Path("C:/Users/lucas/Downloads/script.py")) == "code"


def test_categorize_png_is_image():
    assert categorize(Path("C:/Users/lucas/Downloads/screenshot.png")) == "image"


def test_categorize_mp4_is_media():
    assert categorize(Path("C:/Users/lucas/Videos/clip.mp4")) == "media"


def test_categorize_mp3_is_media():
    assert categorize(Path("C:/Users/lucas/Music/song.mp3")) == "media"


def test_categorize_pdf_is_document():
    assert categorize(Path("C:/Users/lucas/Documents/relatorio.pdf")) == "document"


def test_categorize_exe_is_installer():
    assert categorize(Path("D:/Downloads/setup.exe")) == "installer"


def test_categorize_zip_is_installer():
    assert categorize(Path("D:/Downloads/arquivo.zip")) == "installer"


def test_categorize_msi_is_installer():
    assert categorize(Path("D:/Downloads/pacote.msi")) == "installer"


def test_categorize_unknown_extension_is_none():
    assert categorize(Path("C:/Users/lucas/Downloads/dados.bin")) is None


def test_categorize_is_case_insensitive():
    assert categorize(Path("C:/Users/lucas/Downloads/FOTO.PNG")) == "image"


def test_is_ignored_true_for_node_modules():
    assert is_ignored(Path("C:/Users/lucas/Projects/app/node_modules/pkg/index.js")) is True


def test_is_ignored_true_for_git_dir():
    assert is_ignored(Path("C:/Users/lucas/Projects/app/.git/HEAD")) is True


def test_is_ignored_true_for_windows_temp():
    assert is_ignored(Path("C:/Users/lucas/AppData/Local/Temp/foo.py")) is True


def test_is_ignored_false_for_normal_path():
    assert is_ignored(Path("C:/Users/lucas/Downloads/script.py")) is False
