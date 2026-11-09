from app.server import JOIN_RE, ServerManager, State


def manager():
    m = ServerManager()
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


def test_join_leave_chat_and_list():
    m = manager()
    m._handle_line("[12:00:02] [Server thread/INFO]: There are 1 of a max of 20 players online: Alex")
    m._handle_line("[12:00:05] [Server thread/INFO]: Steve joined the game")
    m._handle_line("[12:00:06] [Server thread/INFO]: <Steve> hallo")
    m._handle_line("[12:00:07 INFO]: [Not Secure] <Alex> hi")
    m._handle_line("[12:00:08] [Server thread/INFO]: Alex left the game")
    assert m.players == {"Steve"}
    assert [c["text"] for c in m.chat] == ["→ Steve joined", "<Steve> hallo", "<Alex> hi", "← Alex left"]


def test_system_chat_prefix_26x():
    m = manager()
    m._handle_line("[18:21:31] [Server thread/INFO]: System chat: There are 1 of a max of 20 players online: Alex")
    m._handle_line("[18:22:21] [Server thread/INFO]: System chat: Steve joined the game")
    m._handle_line("[18:22:30] [Server thread/INFO]: System chat: Alex left the game")
    m._handle_line("[18:22:40] [Server thread/INFO]: System chat: Saved the game")
    assert m.players == {"Steve"}
    assert m._saved_event.is_set()


def test_renamed_player_join():
    assert JOIN_RE.search("[x]: Steve (formerly known as Bob) joined the game").group(1) == "Steve"


def test_chat_message_is_not_a_join():
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
