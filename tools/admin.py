#!/usr/bin/env python3
import argparse
import sqlite3
import sys


def fmt_time(ms):
    if ms is None:
        return "-"
    s = ms / 1000
    m, s = divmod(s, 60)
    return f"{int(m)}:{s:05.2f}"


def print_table(headers, rows):
    if not rows:
        print("(no results)")
        return
    widths = [len(h) for h in headers]
    str_rows = []
    for row in rows:
        cells = [str(v) if v is not None else "-" for v in row]
        str_rows.append(cells)
        for i, c in enumerate(cells):
            widths[i] = max(widths[i], len(c))
    fmt = "  ".join(f"{{:<{w}}}" for w in widths)
    print(fmt.format(*headers))
    print(fmt.format(*["-" * w for w in widths]))
    for row in str_rows:
        print(fmt.format(*row))


def cmd_users(conn, args):
    rows = conn.execute(
        "SELECT user_id, COUNT(*) as games, "
        "SUM(CASE WHEN completed_at IS NOT NULL THEN 1 ELSE 0 END) as completed "
        "FROM games GROUP BY user_id ORDER BY games DESC"
    ).fetchall()
    print_table(["USER_ID", "GAMES", "COMPLETED"], rows)


def cmd_games(conn, args):
    rows = conn.execute(
        "SELECT id, category, target_count, accepted_count, "
        "total_time_ms, started_at, "
        "CASE WHEN completed_at IS NOT NULL THEN 'done' ELSE 'active' END as status "
        "FROM games WHERE user_id = ? ORDER BY started_at DESC",
        (args.user_id,),
    ).fetchall()
    formatted = []
    for r in rows:
        formatted.append((r[0][:12] + "...", r[1], r[2], r[3], fmt_time(r[4]), r[5], r[6]))
    print_table(["GAME_ID", "CAT", "TARGET", "ACCEPTED", "TIME", "STARTED", "STATUS"], formatted)


def cmd_game(conn, args):
    game = conn.execute(
        "SELECT id, user_id, ip_hash, category, target_count, accepted_count, "
        "fallback_lookups_used, started_at, completed_at, total_time_ms "
        "FROM games WHERE id = ?",
        (args.game_id,),
    ).fetchone()
    if not game:
        print(f"Game {args.game_id} not found.")
        sys.exit(1)
    labels = [
        "ID", "User", "IP Hash", "Category", "Target", "Accepted",
        "Fallbacks", "Started", "Completed", "Time",
    ]
    values = list(game)
    values[9] = fmt_time(values[9])
    width = max(len(l) for l in labels)
    for label, val in zip(labels, values):
        print(f"  {label:<{width}}  {val if val is not None else '-'}")

    guesses = conn.execute(
        "SELECT guess_order, name_entered, accepted, guess_time_ms, person_id "
        "FROM guesses WHERE game_id = ? ORDER BY guess_order",
        (args.game_id,),
    ).fetchall()
    if guesses:
        print(f"\n  Guesses ({len(guesses)}):")
        formatted = []
        for g in guesses:
            status = "yes" if g[2] else "no"
            formatted.append((g[0], g[1], status, fmt_time(g[3]), g[4] or "-"))
        print_table(["#", "NAME", "OK", "TIME", "PERSON_ID"], formatted)


def cmd_delete_game(conn, args):
    game = conn.execute("SELECT id, user_id FROM games WHERE id = ?", (args.game_id,)).fetchone()
    if not game:
        print(f"Game {args.game_id} not found.")
        sys.exit(1)
    if not args.yes:
        resp = input(f"Delete game {game[0]} (user: {game[1]})? [y/N] ")
        if resp.lower() != "y":
            print("Aborted.")
            return
    conn.execute("DELETE FROM guesses WHERE game_id = ?", (args.game_id,))
    conn.execute("DELETE FROM games WHERE id = ?", (args.game_id,))
    conn.commit()
    print(f"Deleted game {args.game_id} and its guesses.")


def cmd_leaderboard(conn, args):
    rows = conn.execute(
        "SELECT user_id, total_time_ms, accepted_count, id "
        "FROM games "
        "WHERE completed_at IS NOT NULL AND category = ? AND target_count = ? "
        "ORDER BY total_time_ms ASC LIMIT ?",
        (args.category, args.count, args.limit),
    ).fetchall()
    formatted = []
    for i, r in enumerate(rows, 1):
        formatted.append((i, r[0], fmt_time(r[1]), r[2], r[3][:12] + "..."))
    print_table(["RANK", "USER_ID", "TIME", "ACCEPTED", "GAME_ID"], formatted)


def cmd_rename_user(conn, args):
    count = conn.execute("SELECT COUNT(*) FROM games WHERE user_id = ?", (args.old_id,)).fetchone()[0]
    if count == 0:
        print(f"No games found for user {args.old_id}.")
        sys.exit(1)
    if not args.yes:
        resp = input(f"Rename {args.old_id} -> {args.new_id} ({count} games)? [y/N] ")
        if resp.lower() != "y":
            print("Aborted.")
            return
    conn.execute("UPDATE games SET user_id = ? WHERE user_id = ?", (args.new_id, args.old_id))
    conn.commit()
    print(f"Renamed {count} games from {args.old_id} to {args.new_id}.")


def cmd_stats(conn, args):
    total_games = conn.execute("SELECT COUNT(*) FROM games").fetchone()[0]
    completed = conn.execute("SELECT COUNT(*) FROM games WHERE completed_at IS NOT NULL").fetchone()[0]
    users = conn.execute("SELECT COUNT(DISTINCT user_id) FROM games").fetchone()[0]
    people = conn.execute("SELECT COUNT(*) FROM people").fetchone()[0]
    variants = conn.execute("SELECT COUNT(*) FROM name_variants").fetchone()[0]
    print(f"  Games:     {total_games} ({completed} completed)")
    print(f"  Users:     {users}")
    print(f"  People:    {people}")
    print(f"  Variants:  {variants}")


def main():
    parser = argparse.ArgumentParser(description="Name a Woman - Admin CLI")
    parser.add_argument("--db", default="data/names.db", help="Path to SQLite database")
    sub = parser.add_subparsers(dest="command", required=True)

    sub.add_parser("users", help="List all users with game counts")
    sub.add_parser("stats", help="Show database summary stats")

    p = sub.add_parser("games", help="List games for a user")
    p.add_argument("user_id")

    p = sub.add_parser("game", help="Show game details and guesses")
    p.add_argument("game_id")

    p = sub.add_parser("delete-game", help="Delete a game and its guesses")
    p.add_argument("game_id")
    p.add_argument("-y", "--yes", action="store_true", help="Skip confirmation")

    p = sub.add_parser("rename-user", help="Rename a user ID across all their games")
    p.add_argument("old_id")
    p.add_argument("new_id")
    p.add_argument("-y", "--yes", action="store_true", help="Skip confirmation")

    p = sub.add_parser("leaderboard", help="Show top completions")
    p.add_argument("--category", default="women", choices=["women", "men", "people"])
    p.add_argument("--count", type=int, default=100)
    p.add_argument("--limit", type=int, default=20)

    args = parser.parse_args()
    conn = sqlite3.connect(args.db)

    cmds = {
        "users": cmd_users,
        "games": cmd_games,
        "game": cmd_game,
        "delete-game": cmd_delete_game,
        "rename-user": cmd_rename_user,
        "leaderboard": cmd_leaderboard,
        "stats": cmd_stats,
    }
    cmds[args.command](conn, args)
    conn.close()


if __name__ == "__main__":
    main()
