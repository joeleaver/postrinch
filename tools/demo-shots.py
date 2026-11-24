#!/usr/bin/env python3
"""Take the website's screenshots from a demo window.

    tools/demo-window.sh --debug <scratch>          # light; prints its pid
    tools/demo-shots.py take <pid> site/public/assets/shots
    tools/demo-window.sh --dark --debug <scratch>
    tools/demo-shots.py take <pid> site/public/assets/shots --suffix -dark

It drives the window through rinch's debug server, as a person would:
clicks on elements found by their text, typing and keys, never anything
the window could not do itself. Each shot is the 1440x900 window as it
rendered. `find <pid> <selector> [text]` lists matching elements, for
working out a new step.

Only the standard library; the debug server's protocol is a 4-byte
big-endian length, then JSON.
"""

import argparse
import base64
import json
import os
import socket
import struct
import sys
import time

# The window's size, and so every shot's.
WIDTH, HEIGHT = 1440, 900


class Window:
    """One demo window's debug server, as a context manager that closes
    the connection however the steps end."""

    def __init__(self, pid):
        with open(os.path.expanduser(f"~/.rinch/debug/{pid}.json")) as f:
            found = json.load(f)
        # A window that stops answering fails the step, rather than
        # leaving the script waiting for good.
        self.sock = socket.create_connection(("127.0.0.1", found["port"]), timeout=30)
        self.next_id = 0
        try:
            self._send({"protocol": "rinch-debug", "version": 1, "token": found["token"]})
            reply = self._recv()
        except BaseException:
            self.sock.close()
            raise
        if isinstance(reply, dict) and reply.get("error"):
            self.sock.close()
            raise ConnectionRefusedError(f"the window refused the connection: {reply['error']}")

    def __enter__(self):
        return self

    def __exit__(self, *_):
        self.sock.close()

    def _send(self, obj):
        data = json.dumps(obj).encode()
        self.sock.sendall(struct.pack(">I", len(data)) + data)

    def _exact(self, n):
        out = b""
        while len(out) < n:
            chunk = self.sock.recv(n - len(out))
            if not chunk:
                raise EOFError("the window closed the connection")
            out += chunk
        return out

    def _recv(self):
        (n,) = struct.unpack(">I", self._exact(4))
        return json.loads(self._exact(n))

    def call(self, method, params=None):
        self.next_id += 1
        request = {"id": self.next_id, "method": method}
        if params is not None:
            request["params"] = params
        self._send(request)
        reply = self._recv()
        if reply.get("type") == "error":
            raise RuntimeError(f"{method}: {reply.get('message')}")
        if reply.get("type") == "bytes":
            return base64.b64decode(reply["data"])
        return reply.get("data")

    def find(self, selector, text=None):
        """Elements matching `selector` whose text contains `text`, in
        document order: those shown, as a hidden one has no size, and
        whose middle is inside the window, where a click would reach it."""
        found = self.call("query_selector", {"selector": selector}) or []
        return [
            e
            for e in found
            if shown(e["absolute"])
            and (text is None or text in (e.get("text_content") or ""))
        ]

    def click(self, selector, text=None, nth=0, settle=0.8):
        """Click the middle of the nth element matching (from the end when
        negative), waiting for it to appear (up to five seconds)."""
        for _ in range(25):
            found = self.find(selector, text)
            if len(found) >= (nth + 1 if nth >= 0 else -nth):
                box = found[nth]["absolute"]
                x = box["x"] + box["width"] / 2
                y = box["y"] + box["height"] / 2
                self.call("click", {"x": x, "y": y})
                time.sleep(settle)
                return
            time.sleep(0.2)
        raise LookupError(f"nothing matches {selector!r} with {text!r}")

    def type(self, text):
        self.call("type_text", {"text": text})

    def key(self, key):
        self.call("key_press", {"key": key, "shift": False, "ctrl": False, "alt": False})
        time.sleep(0.8)

    def shot(self, path):
        """Write the window as it is to `path`, once it is checked to be a
        PNG of the window's size: a wrong one is never written."""
        png = self.call("screenshot")
        if png[:8] != b"\x89PNG\r\n\x1a\n":
            raise RuntimeError(f"the screenshot for {path} is not a PNG")
        width, height = struct.unpack(">II", png[16:24])
        if (width, height) != (WIDTH, HEIGHT):
            raise RuntimeError(f"the screenshot for {path} is {width}x{height}, not {WIDTH}x{HEIGHT}")
        with open(path, "wb") as f:
            f.write(png)
        print(f"wrote {path}")


def shown(box):
    """Whether an element's box has a size and its middle is in the window."""
    x = box["x"] + box["width"] / 2
    y = box["y"] + box["height"] / 2
    return box["width"] > 0 and box["height"] > 0 and 0 <= x < WIDTH and 0 <= y < HEIGHT


def settings(w, section):
    """Open Settings from the File menu, at `section`."""
    w.click(".rinch-app-menu-item__label", "File")
    w.click(".rinch-app-menu-entry", "Settings…")
    w.click(".settings-nav .folder", section)


def take(w, out, suffix):
    """Take the six shots, in an order where each starts from where the
    last left the window."""

    def path(name):
        return os.path.join(out, f"{name}{suffix}.png")

    # Mail: the client thread, the message with its PDF open too.
    w.click(".list .row", "Harbor Coffee rebrand: round 2 feedback")
    w.click(".message-folded", "Here are our notes")
    w.shot(path("mail"))

    # Views: the person's Harbor Coffee view, its edit panel open beside
    # the list it shows.
    w.click(".view-row", "Harbor Coffee")
    w.click("button.list-edit")
    w.shot(path("views"))
    w.click("button", "Done")

    # Search: mail with files this month, a receipt open.
    w.click("input[placeholder='Search all mail']")
    w.type("has:attachment newer_than:30d")
    w.key("Enter")
    w.click(".list .row", "Your receipt from Paper Crane Shop")
    w.shot(path("search"))
    w.click(".unsaved .row-close")

    # Calendar: this week, the workshop the person hasn't answered open.
    w.click("button.mode", "Calendar", settle=1.5)
    w.click(".cal-event", "Harbor Coffee: packaging workshop")
    w.shot(path("calendar"))

    # Rules: the work account's Harbor Coffee rule, in its editor.
    settings(w, "Rules")
    w.click(".rules-chip", "sam@lindenworks.example")
    w.click(".rule-name", "Harbor Coffee", settle=1.5)
    w.shot(path("rules"))
    w.click(".rinch-modal button", "Cancel")

    # Agents: Claude connected from here, as the person would.
    w.click(".settings-nav .folder", "Agents")
    w.click("button", "Connect")
    w.click(".rinch-modal button", "Connect", nth=-1, settle=1.5)
    w.shot(path("agents"))


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    sub = parser.add_subparsers(dest="command", required=True)
    take_cmd = sub.add_parser("take", help="take every shot")
    take_cmd.add_argument("pid")
    take_cmd.add_argument("out", help="the folder the shots go in")
    take_cmd.add_argument("--suffix", default="", help="after each name, e.g. -dark")
    take_cmd.add_argument("--close", action="store_true", help="close the window after")
    find_cmd = sub.add_parser("find", help="list elements matching a selector")
    find_cmd.add_argument("pid")
    find_cmd.add_argument("selector")
    find_cmd.add_argument("text", nargs="?")
    args = parser.parse_args()
    with Window(args.pid) as w:
        run(w, args)


def run(w, args):
    """Do what the command line asks, through the window `w`."""
    if args.command == "find":
        for e in w.find(args.selector, args.text):
            box = e["absolute"]
            attrs = {k: v for k, v in e.get("attributes", {}).items() if not k.startswith("data-on")}
            text = (e.get("text_content") or "")[:60]
            print(f"{e.get('tag')} {attrs} {text!r} at ({box['x']:.0f},{box['y']:.0f}) "
                  f"{box['width']:.0f}x{box['height']:.0f}")
        return
    os.makedirs(args.out, exist_ok=True)
    take(w, args.out, args.suffix)
    if args.close:
        w.call("close_app")


if __name__ == "__main__":
    sys.exit(main())
