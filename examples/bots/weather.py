#!/usr/bin/env python3
"""A Liveside bot: the weather on request, from wttr.in.

    !pocasi [město]        !weather [city]
    !pocasi_help           !weather_help

Liveside starts it and talks to it in JSON lines; see docs/bots.md.
"""

import json
import sys
import urllib.parse
import urllib.request

DEFAULT_CITY = "Praha"
HELP = (
    "Napiš !pocasi [město] nebo !weather [city], např. '!pocasi Brno'. "
    f"Výchozí město je {DEFAULT_CITY}."
)


def weather(city):
    url = "https://wttr.in/" + urllib.parse.quote(city) + "?format=4"
    try:
        with urllib.request.urlopen(url, timeout=10) as response:
            text = response.read().decode("utf-8").strip()
    except OSError as e:
        print(f"wttr.in: {e}", file=sys.stderr)
        return "Počasí se teď nepodařilo zjistit, zkus to za chvíli."
    if "Unknown location" in text or not text:
        return f"Nepodařilo se najít počasí pro '{city}'."
    return text


def answer(text):
    command, _, rest = text.strip().partition(" ")
    command = command.lower()
    if command in ("!pocasi", "!weather"):
        return weather(rest.strip() or DEFAULT_CITY)
    if command in ("!pocasi_help", "!weather_help"):
        return HELP
    return None


def main():
    for line in sys.stdin:
        try:
            event = json.loads(line)
        except ValueError:
            continue
        if event.get("type") != "message":
            continue
        reply = answer(event.get("text", ""))
        if reply is None:
            continue
        print(
            json.dumps(
                {
                    "type": "reply",
                    "channel": event["channel"],
                    "to": event["id"],
                    "text": reply,
                },
                ensure_ascii=False,
            ),
            flush=True,
        )


if __name__ == "__main__":
    main()
