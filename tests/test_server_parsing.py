from app.server import JOIN_RE, ServerManager, State
from app.instances import MAIN


def manager():
    m = ServerManager(MAIN)
    m.sent = []
    m.send_command = m.sent.append
    return m


def test_done_switches_to_running_and_asks_for_player_list(data):
    m = manager()
    m.state = State.STARTING
    m.running_version, m.running_flavor = "26.3", "paper"
    m._handle_line('[12:00:01 INFO]: Done (3.2s)! For help, type "help"')
    assert m.state == State.RUNNING
    assert m.sent == ["list"]


def test_join_leave_chat_and_list(data):
    m = manager()
    m._handle_line("[12:00:02] [Server thread/INFO]: There are 1 of a max of 20 players online: Alex")
    m._handle_line("[12:00:05] [Server thread/INFO]: Steve joined the game")
    m._handle_line("[12:00:06] [Server thread/INFO]: <Steve> hallo")
    m._handle_line("[12:00:07 INFO]: [Not Secure] <Alex> hi")
    m._handle_line("[12:00:08] [Server thread/INFO]: Alex left the game")
    assert m.players == {"Steve"}
    assert [c["text"] for c in m.chat] == ["→ Steve joined", "<Steve> hallo", "<Alex> hi", "← Alex left"]


def test_system_chat_prefix_26x(data):
    m = manager()
    m._handle_line("[18:21:31] [Server thread/INFO]: System chat: There are 1 of a max of 20 players online: Alex")
    m._handle_line("[18:22:21] [Server thread/INFO]: System chat: Steve joined the game")
    m._handle_line("[18:22:30] [Server thread/INFO]: System chat: Alex left the game")
    m._handle_line("[18:22:40] [Server thread/INFO]: System chat: Saved the game")
    assert m.players == {"Steve"}
    assert m._saved_event.is_set()


def test_renamed_player_join():
    assert JOIN_RE.search("[x]: Steve (formerly known as Bob) joined the game").group(1) == "Steve"


def test_chat_message_is_not_a_join(data):
    m = manager()
    m._handle_line("[x INFO]: <Bob> Steve joined the game")
    assert m.players == set()


def test_whitelist_rejections_newest_first_without_duplicates():
    m = manager()
    for name in ("Bob", "Eve", "Bob"):
        m._handle_line(f"[x INFO]: Disconnecting {name} (/1.2.3.4:5): You are not whitelisted on this server!")
    m._handle_line("[x] [Server thread/INFO]: Disconnecting Alex (/1:2): You are not white-listed on this server!")
    assert m.rejected == ["Alex", "Bob", "Eve"]


def test_saved_the_game_releases_live_backup():
    m = manager()
    m._handle_line("[x INFO]: Saved the game")
    assert m._saved_event.is_set()


# ---------- panel queries (answers are swallowed) ----------


def answering(m, replies):
    """send_command that feeds the reply lines of each command back into the parser."""
    def send(cmd):
        m.sent.append(cmd)
        for line in replies.get(cmd, []):
            m._handle_line(line)
    m.send_command = send


def test_vanilla_tick_query_is_parsed_and_swallowed(data):
    m = manager()
    m.state, m.running_flavor, m.running_version = State.RUNNING, "vanilla", "1.21.4"
    answering(m, {"tick query": [
        "[x] [Server thread/INFO]: The game is running normally",
        "[x] [Server thread/INFO]: Target tick rate: 20.0 per second.",
        "[x] [Server thread/INFO]: Average time per tick: 12.5ms (Target: 50.0ms)",
        "[x] [Server thread/INFO]: Percentiles: P50: 10.0ms P95: 20.0ms P99: 30.0ms, sample: 100",
    ]})
    t = m.query_tick()
    assert t["mspt"] == 12.5 and t["tps"] == 20.0
    assert not m.log  # nothing reached the console


def test_slow_server_tps_below_target(data):
    m = manager()
    m.state, m.running_flavor, m.running_version = State.RUNNING, "fabric", "26.3"
    answering(m, {"tick query": [
        "[x] [Server thread/INFO]: System chat: The game is running normally",
        "[x] [Server thread/INFO]: System chat: Target tick rate: 20.0 per second.",
        "[x] [Server thread/INFO]: System chat: Average time per tick: 80.0ms (Target: 50.0ms)",
        "[x] [Server thread/INFO]: System chat: Percentiles: P50: 1ms",
    ]})
    assert m.query_tick()["tps"] == 12.5


def test_paper_tps_and_mspt(data):
    m = manager()
    m.state, m.running_flavor, m.running_version = State.RUNNING, "paper", "26.3"
    answering(m, {
        "tps": ["[12:00:00 INFO]: TPS from last 1m, 5m, 15m: *20.0, 19.5, 19.9"],
        "mspt": ["[12:00:00 INFO]: Server tick times (avg/min/max) from last 5s, 10s, 1m:",
                 "[12:00:00 INFO]: ◴ 3.1/1.0/9.9, 3.0/1.0/9.9, 3.2/0.9/12.0"],
    })
    t = m.query_tick()
    assert t == {"tps": 20.0, "mspt": 3.1, "t": t["t"]}
    assert not m.log


def test_old_vanilla_has_no_tick_command(data):
    m = manager()
    m.state, m.running_flavor, m.running_version = State.RUNNING, "vanilla", "1.20.2"
    assert m.query_tick() is None and m.sent == []


def test_unrelated_lines_still_reach_the_console(data):
    m = manager()
    m.state, m.running_flavor, m.running_version = State.RUNNING, "vanilla", "1.21.4"
    answering(m, {"tick query": ["[x] [Server thread/INFO]: Steve joined the game"]})
    assert m.ask(["tick query"], __import__("app.server", fromlist=["x"]).TICK_LINE_RE, 1, timeout=0.1) == []
    assert m.players == {"Steve"}


def test_join_and_leave_are_recorded(data):
    from app import players
    m = manager()
    m._handle_line("[x] [Server thread/INFO]: Steve joined the game")
    s = players.seen(MAIN, "steve")
    assert s["online"] and s["first_seen"] == s["last_seen"]
    m._handle_line("[x] [Server thread/INFO]: Steve left the game")
    assert players.seen(MAIN, "Steve")["online"] is False
