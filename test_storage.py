from storage import connect, query_recent, upsert_file


def make_conn(tmp_path):
    return connect(tmp_path / "history.db")


def test_upsert_then_query_returns_the_file(tmp_path):
    conn = make_conn(tmp_path)

    upsert_file(conn, path="C:/Users/x/Downloads/a.py", category="code", size=10, created_at=100.0, seen_at=100.0)

    rows = query_recent(conn)
    assert len(rows) == 1
    assert rows[0]["path"] == "C:/Users/x/Downloads/a.py"
    assert rows[0]["category"] == "code"


def test_upsert_same_path_twice_updates_not_duplicates(tmp_path):
    conn = make_conn(tmp_path)

    upsert_file(conn, path="C:/Users/x/Downloads/a.py", category="code", size=10, created_at=100.0, seen_at=100.0)
    upsert_file(conn, path="C:/Users/x/Downloads/a.py", category="code", size=20, created_at=100.0, seen_at=200.0)

    rows = query_recent(conn)
    assert len(rows) == 1
    assert rows[0]["size"] == 20
    assert rows[0]["seen_at"] == 200.0


def test_query_recent_orders_most_recent_first(tmp_path):
    conn = make_conn(tmp_path)
    upsert_file(conn, path="a.py", category="code", size=1, created_at=1.0, seen_at=1.0)
    upsert_file(conn, path="b.py", category="code", size=1, created_at=2.0, seen_at=2.0)

    rows = query_recent(conn)

    assert [r["path"] for r in rows] == ["b.py", "a.py"]


def test_query_recent_filters_by_category(tmp_path):
    conn = make_conn(tmp_path)
    upsert_file(conn, path="a.py", category="code", size=1, created_at=1.0, seen_at=1.0)
    upsert_file(conn, path="b.png", category="image", size=1, created_at=2.0, seen_at=2.0)

    rows = query_recent(conn, categories=["image"])

    assert [r["path"] for r in rows] == ["b.png"]


def test_query_recent_filters_by_search_substring(tmp_path):
    conn = make_conn(tmp_path)
    upsert_file(conn, path="C:/Downloads/report.pdf", category="document", size=1, created_at=1.0, seen_at=1.0)
    upsert_file(conn, path="C:/Downloads/notes.pdf", category="document", size=1, created_at=2.0, seen_at=2.0)

    rows = query_recent(conn, search="report")

    assert [r["path"] for r in rows] == ["C:/Downloads/report.pdf"]
