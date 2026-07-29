#!/usr/bin/env python3
"""Browserless BOSS Zhipin session transport used by the Rust CLI.

The process reads one JSON request from stdin and writes one JSON response to
stdout. Credential values are returned only to the parent process and are
never included in diagnostics.
"""

from __future__ import annotations

import json
import os
import secrets
import ssl
import sys
import time
from http.cookies import SimpleCookie
from threading import Event
from urllib.parse import quote

FRIEND_LIST_URL = (
    "https://www.zhipin.com/wapi/zprelation/friend/getGeekFriendList.json"
)
FRIEND_ADD_URL = "https://www.zhipin.com/wapi/zpgeek/friend/add.json"
API_URL = "https://www.zhipin.com/wapi/zpgeek/search/joblist.json"
USER_INFO_URL = "https://www.zhipin.com/wapi/zpuser/wap/getUserInfo.json"
WT_URL = "https://www.zhipin.com/wapi/zppassport/get/wt"
HISTORY_URL = "https://www.zhipin.com/wapi/zpchat/geek/historyMsg"
BASE_URL = "https://www.zhipin.com"
MAX_LOOKUP_PAGES = 3
LOOKUP_PAGE_SIZE = 30
HISTORY_PAGE_SIZE = 20
MAX_MESSAGE_CHARS = 200
MAX_HISTORY_MESSAGES = 20
MAX_HISTORY_TEXT_CHARS = 2000
MAX_HISTORY_RESPONSE_BYTES = 60 * 1024
MAX_INBOX_CONVERSATIONS = 5
MAX_INBOX_TEXT_CHARS = 512
MAX_INBOX_RESPONSE_BYTES = 60 * 1024
MAX_REMOTE_ID_CHARS = 2048
MAX_OPAQUE_VALUE_CHARS = 4096
SEND_DEADLINE_SECONDS = 50.0
USER_AGENT = (
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64) "
    "AppleWebKit/537.36 (KHTML, like Gecko) "
    "Chrome/124.0.0.0 Safari/537.36"
)
HEADERS = {
    "User-Agent": USER_AGENT,
    "Accept": "application/json, text/plain, */*",
    "Content-Type": "application/x-www-form-urlencoded",
    "Origin": BASE_URL,
    "Referer": f"{BASE_URL}/web/geek/jobs?query=AI%20Agent",
    "X-Requested-With": "XMLHttpRequest",
}
SEARCH_DATA = {
    "scene": "1",
    "query": "AI Agent",
    "city": "",
    "page": "1",
    "pageSize": "1",
}


class SafeFailure(Exception):
    """An expected failure whose message contains no credential material."""


def request_timeout(deadline: float | None) -> float:
    if deadline is None:
        return 15.0
    remaining = deadline - time.monotonic()
    if remaining <= 0.25:
        raise SafeFailure("Zhipin direct message operation timed out")
    return min(5.0, remaining)


def cookie_pairs(header: str) -> list[tuple[str, str]]:
    parsed = SimpleCookie()
    parsed.load(header)
    pairs = [(name, morsel.value) for name, morsel in parsed.items()]
    if not pairs:
        raise SafeFailure("stored Zhipin Cookie is invalid")
    return pairs


def replace_cookie(
    pairs: list[tuple[str, str]], name: str, value: str
) -> list[tuple[str, str]]:
    updated = [(key, item) for key, item in pairs if key != name]
    updated.append((name, value))
    return updated


def cookie_header(pairs: list[tuple[str, str]]) -> str:
    return "; ".join(f"{name}={value}" for name, value in pairs)


def request_json(response: requests.Response, action: str) -> dict:
    try:
        payload = response.json()
    except ValueError as error:
        raise SafeFailure(f"{action} returned invalid JSON") from error
    if not isinstance(payload, dict):
        raise SafeFailure(f"{action} returned an invalid payload")
    return payload


def api_code(payload: dict, action: str) -> int:
    code = payload.get("code")
    if type(code) is not int:
        raise SafeFailure(f"{action} returned an invalid API code")
    return code


def challenge_token(
    seed: str, name: str, timestamp: int, deadline: float | None
) -> str:
    import requests

    saved_stdout = os.dup(sys.stdout.fileno())
    try:
        with open(os.devnull, "w", encoding="utf-8") as sink:
            sys.stdout.flush()
            os.dup2(sink.fileno(), sys.stdout.fileno())
            import iv8
            sys.stdout.flush()
    finally:
        os.dup2(saved_stdout, sys.stdout.fileno())
        os.close(saved_stdout)

    if (
        not seed
        or len(seed) > 4096
        or not name
        or len(name) > 128
        or not name.isascii()
        or not all(character.isalnum() or character in "_-" for character in name)
        or type(timestamp) is not int
        or timestamp <= 0
        or timestamp > 2**63 - 1
    ):
        raise SafeFailure("Zhipin security challenge is incomplete")
    js_url = f"{BASE_URL}/web/common/security-js/{name}.js"
    response = requests.get(
        js_url,
        headers={"User-Agent": USER_AGENT},
        timeout=request_timeout(deadline),
    )
    if response.status_code != 200 or not response.text:
        raise SafeFailure("unable to load Zhipin security challenge")

    security_url = (
        f"{BASE_URL}/web/common/security-check.html"
        f"?seed={quote(seed, safe='')}&name={name}&ts={timestamp}&callbackUrl=&srcReferer"
    )
    environment = {
        "location": {
            "href": security_url,
            "origin": BASE_URL,
            "protocol": "https:",
            "host": "www.zhipin.com",
            "hostname": "www.zhipin.com",
            "port": "",
            "pathname": "/web/common/security-check.html",
            "search": "?" + security_url.split("?", 1)[1],
            "hash": "",
        },
        "window": {"origin": BASE_URL},
    }
    html = (
        "<!DOCTYPE html><html><head></head><body>"
        f'<script src="{js_url}"></script></body></html>'
    )
    with iv8.JSContext(
        environment=environment,
        config={"timezone": "Asia/Shanghai"},
    ) as context:
        context.expose(
            {
                "baseURL": security_url,
                "html": html,
                "headers": [],
                "resources": {js_url: response.text},
            },
            "snapshot",
        )
        context.eval("__iv8__.page.load(__iv8__.data.snapshot)")
        encoded_seed = json.dumps(seed, ensure_ascii=True)
        token = context.eval(
            f"encodeURIComponent((new window.ABC).z({encoded_seed}, {timestamp}));"
        )
    if not isinstance(token, str) or not token:
        raise SafeFailure("Zhipin security challenge produced no token")
    return token


def apply_challenge(
    payload: dict,
    pairs: list[tuple[str, str]],
    session: requests.Session,
    deadline: float | None,
) -> list[tuple[str, str]]:
    challenge = payload.get("zpData")
    if not isinstance(challenge, dict):
        raise SafeFailure("Zhipin security challenge is missing")
    seed = challenge.get("seed")
    name = challenge.get("name")
    timestamp = challenge.get("ts")
    if not isinstance(seed, str) or not isinstance(name, str):
        raise SafeFailure("Zhipin security challenge is incomplete")
    token = challenge_token(seed, name, timestamp, deadline)
    pairs = replace_cookie(pairs, "__zp_stoken__", token)
    session.cookies.set(
        "__zp_stoken__",
        token,
        domain=".zhipin.com",
        path="/",
    )
    return pairs


def friend_list(
    session: requests.Session, page: int, deadline: float | None = None
) -> dict:
    return request_json(
        session.get(
            FRIEND_LIST_URL,
            headers=HEADERS,
            params={"page": str(page)},
            timeout=request_timeout(deadline),
        ),
        "Zhipin friend list",
    )


def prepare_session(
    cookie: str,
    deadline: float | None = None,
) -> tuple[requests.Session, list[tuple[str, str]], bool]:
    import requests

    pairs = cookie_pairs(cookie)
    session = requests.Session()
    for name, value in pairs:
        session.cookies.set(name, value, domain=".zhipin.com", path="/")

    initial = request_json(
        session.post(
            API_URL,
            headers=HEADERS,
            data=SEARCH_DATA,
            timeout=request_timeout(deadline),
        ),
        "Zhipin session check",
    )
    code = api_code(initial, "Zhipin session check")
    token_refreshed = False
    if code == 37:
        pairs = apply_challenge(initial, pairs, session, deadline)
        token_refreshed = True
        verified = request_json(
            session.post(
                API_URL,
                headers=HEADERS,
                data=SEARCH_DATA,
                timeout=request_timeout(deadline),
            ),
            "Zhipin refreshed security check",
        )
        verified_code = api_code(verified, "Zhipin refreshed security check")
        if verified_code != 0:
            raise SafeFailure(
                f"Zhipin refreshed security check failed with API code {verified_code!r}"
            )
    elif code != 0:
        raise SafeFailure(f"Zhipin session check failed with API code {code!r}")

    authenticated = friend_list(session, 1, deadline)
    authenticated_code = api_code(authenticated, "Zhipin authenticated session check")
    if authenticated_code == 37:
        pairs = apply_challenge(authenticated, pairs, session, deadline)
        token_refreshed = True
        authenticated = friend_list(session, 1, deadline)
        authenticated_code = api_code(
            authenticated,
            "Zhipin authenticated session check",
        )
    if authenticated_code != 0:
        raise SafeFailure(
            f"Zhipin authenticated session check failed with API code {authenticated_code!r}"
        )
    return session, pairs, token_refreshed


def refresh(cookie: str) -> dict:
    _, pairs, token_refreshed = prepare_session(cookie)
    return {
        "ok": True,
        "action": "refresh",
        "verification": (
            "security_token_refreshed_and_authenticated_api_code_0"
            if token_refreshed
            else "authenticated_api_code_0"
        ),
        "updated_cookie": cookie_header(pairs),
    }


def result_items(payload: dict, action: str) -> list[dict]:
    data = payload.get("zpData")
    if not isinstance(data, dict):
        raise SafeFailure(f"{action} returned no result data")
    items = data.get("result")
    if items is None and not data:
        return []
    if not isinstance(items, list):
        raise SafeFailure(f"{action} returned an invalid result list")
    if not all(isinstance(item, dict) for item in items):
        raise SafeFailure(f"{action} returned invalid result entries")
    return items


def friend_job_id(item: dict) -> str | None:
    direct = item.get("encryptJobId")
    if isinstance(direct, str) and direct:
        return direct
    for key in ("jobInfo", "jobBaseInfo"):
        nested = item.get(key)
        if isinstance(nested, dict):
            value = nested.get("encryptJobId")
            if isinstance(value, str) and value:
                return value
    return None


def exact_friend(
    session: requests.Session,
    remote_id: str,
    deadline: float | None = None,
) -> dict | None:
    match: dict | None = None
    for page in range(1, MAX_LOOKUP_PAGES + 1):
        payload = friend_list(session, page, deadline)
        code = api_code(payload, "Zhipin friend list")
        if code != 0:
            raise SafeFailure(f"Zhipin friend list failed with API code {code!r}")
        items = result_items(payload, "Zhipin friend list")
        for item in items:
            if friend_job_id(item) != remote_id:
                continue
            if match is not None:
                raise SafeFailure("Zhipin conversation lookup was ambiguous")
            match = item
        if not items:
            break
    return match


def validate_inbox_remote_ids(value: object) -> list[str]:
    if type(value) is not list or not 1 <= len(value) <= MAX_INBOX_CONVERSATIONS:
        raise SafeFailure("chat inbox requires between 1 and 5 jobs")
    remote_ids: list[str] = []
    seen: set[str] = set()
    for item in value:
        if (
            type(item) is not str
            or not item
            or len(item) > MAX_REMOTE_ID_CHARS
            or not item.isprintable()
            or item in seen
        ):
            raise SafeFailure(
                "chat inbox requires unique valid Zhipin job identifiers"
            )
        seen.add(item)
        remote_ids.append(item)
    return remote_ids


def exact_friends(
    session: requests.Session,
    remote_ids: list[str],
    deadline: float,
) -> list[dict]:
    requested = set(remote_ids)
    matches: dict[str, dict] = {}
    for page in range(1, MAX_LOOKUP_PAGES + 1):
        payload = friend_list(session, page, deadline)
        code = api_code(payload, "Zhipin friend list")
        if code != 0:
            raise SafeFailure(f"Zhipin friend list failed with API code {code!r}")
        items = result_items(payload, "Zhipin friend list")
        for item in items:
            remote_id = friend_job_id(item)
            if remote_id not in requested:
                continue
            if remote_id in matches:
                raise SafeFailure("Zhipin conversation lookup was ambiguous")
            matches[remote_id] = item
        if not items:
            break
    if len(matches) != len(remote_ids):
        raise SafeFailure("chat inbox requires existing exact Zhipin conversations")
    return [matches[remote_id] for remote_id in remote_ids]


def has_exact_friend(session: requests.Session, remote_id: str) -> bool:
    return exact_friend(session, remote_id) is not None


def search_exact_job(
    session: requests.Session,
    pairs: list[tuple[str, str]],
    title: str,
    remote_id: str,
) -> tuple[str, str, list[tuple[str, str]]]:
    matches: list[dict] = []
    challenge_applied = False
    for page in range(1, MAX_LOOKUP_PAGES + 1):
        data = {
            "scene": "1",
            "query": title,
            "city": "",
            "page": str(page),
            "pageSize": str(LOOKUP_PAGE_SIZE),
        }
        payload = request_json(
            session.post(API_URL, headers=HEADERS, data=data, timeout=15),
            "Zhipin target search",
        )
        code = api_code(payload, "Zhipin target search")
        if code == 37:
            if challenge_applied:
                raise SafeFailure(
                    "Zhipin target search repeated the security challenge"
                )
            pairs = apply_challenge(payload, pairs, session, None)
            challenge_applied = True
            payload = request_json(
                session.post(API_URL, headers=HEADERS, data=data, timeout=15),
                "Zhipin target search",
            )
            code = api_code(payload, "Zhipin target search")
        if code != 0:
            raise SafeFailure(f"Zhipin target search failed with API code {code!r}")
        response_data = payload.get("zpData")
        if not isinstance(response_data, dict):
            raise SafeFailure("Zhipin target search returned no result data")
        items = response_data.get("jobList")
        if not isinstance(items, list) or not all(
            isinstance(item, dict) for item in items
        ):
            raise SafeFailure("Zhipin target search returned an invalid job list")
        matches.extend(
            item for item in items if item.get("encryptJobId") == remote_id
        )
        if len(items) < LOOKUP_PAGE_SIZE:
            break

    if len(matches) != 1:
        raise SafeFailure("cached Zhipin job could not be resolved exactly")
    security_id = matches[0].get("securityId")
    lid = matches[0].get("lid")
    if not isinstance(security_id, str) or not security_id:
        raise SafeFailure("resolved Zhipin job has no greeting authorization")
    if not isinstance(lid, str) or not lid:
        raise SafeFailure("resolved Zhipin job has no greeting lookup identifier")
    return security_id, lid, pairs


def greet(cookie: str, title: str, remote_id: str) -> dict:
    session, pairs, _ = prepare_session(cookie)
    if has_exact_friend(session, remote_id):
        return {
            "ok": True,
            "action": "greet",
            "state": "already_connected",
            "verification": "exact_encrypt_job_id_in_friend_list",
            "updated_cookie": cookie_header(pairs),
        }

    security_id, lid, pairs = search_exact_job(
        session,
        pairs,
        title,
        remote_id,
    )
    added = request_json(
        session.get(
            FRIEND_ADD_URL,
            headers=HEADERS,
            params={"securityId": security_id, "lid": lid},
            timeout=15,
        ),
        "Zhipin greeting",
    )
    code = api_code(added, "Zhipin greeting")
    if code != 0:
        raise SafeFailure(f"Zhipin greeting failed with API code {code!r}")
    if not has_exact_friend(session, remote_id):
        raise SafeFailure("Zhipin greeting could not be verified")
    return {
        "ok": True,
        "action": "greet",
        "state": "greeting_verified",
        "verification": "exact_encrypt_job_id_in_friend_list",
        "updated_cookie": cookie_header(pairs),
    }


def normalize_message(message: str) -> str:
    normalized = message.strip()
    if (
        not normalized
        or len(normalized) > MAX_MESSAGE_CHARS
        or "\n" in normalized
        or "\r" in normalized
        or not normalized.isprintable()
    ):
        raise SafeFailure(
            "chat message must contain 1 to 200 printable single-line characters"
        )
    return normalized


def positive_int(value: object, field: str) -> int:
    if type(value) is not int or value <= 0 or value > 2**63 - 1:
        raise SafeFailure(f"Zhipin {field} is invalid")
    return value


def exact_int(value: object, field: str) -> int:
    if type(value) is not int or value < 0 or value > 2**31 - 1:
        raise SafeFailure(f"Zhipin {field} is invalid")
    return value


def bounded_string(value: object, field: str) -> str:
    if (
        not isinstance(value, str)
        or not value
        or len(value) > MAX_OPAQUE_VALUE_CHARS
        or not value.isprintable()
    ):
        raise SafeFailure(f"Zhipin {field} is invalid")
    return value


def response_data(payload: dict, action: str) -> dict:
    if api_code(payload, action) != 0:
        raise SafeFailure(f"{action} failed")
    data = payload.get("zpData")
    if not isinstance(data, dict):
        raise SafeFailure(f"{action} returned no result data")
    return data


def history_messages(
    session: requests.Session,
    boss_id: str,
    deadline: float,
) -> list[dict]:
    messages: list[dict] = []
    for page_number in range(1, MAX_LOOKUP_PAGES + 1):
        payload = request_json(
            session.get(
                HISTORY_URL,
                headers=HEADERS,
                params={
                    "bossId": boss_id,
                    "c": str(HISTORY_PAGE_SIZE),
                    "page": str(page_number),
                },
                timeout=request_timeout(deadline),
            ),
            "Zhipin chat history",
        )
        data = response_data(payload, "Zhipin chat history")
        page = data.get("messages")
        if page is None and not data:
            page = []
        if not isinstance(page, list) or not all(
            isinstance(item, dict) for item in page
        ):
            raise SafeFailure("Zhipin chat history returned invalid messages")
        messages.extend(page)
        if len(page) < HISTORY_PAGE_SIZE:
            break
    return messages


def recent_history_page(
    session: requests.Session,
    boss_id: str,
    deadline: float,
) -> list[dict]:
    payload = request_json(
        session.get(
            HISTORY_URL,
            headers=HEADERS,
            params={
                "bossId": boss_id,
                "c": str(HISTORY_PAGE_SIZE),
                "page": "1",
            },
            timeout=request_timeout(deadline),
        ),
        "Zhipin chat history",
    )
    data = response_data(payload, "Zhipin chat history")
    messages = data.get("messages")
    if messages is None and not data:
        return []
    if not isinstance(messages, list) or not all(
        isinstance(item, dict) for item in messages
    ):
        raise SafeFailure("Zhipin chat history returned invalid messages")
    return messages


def has_exact_outgoing_text(
    messages: list[dict], user_id: int, message: str
) -> bool:
    for item in messages:
        sender = item.get("from")
        body = item.get("body")
        if (
            isinstance(sender, dict)
            and isinstance(body, dict)
            and type(sender.get("uid")) is int
            and sender["uid"] == user_id
            and isinstance(body.get("text"), str)
            and body["text"] == message
        ):
            return True
    return False


def unsafe_history_character(character: str) -> bool:
    codepoint = ord(character)
    return (
        codepoint < 0x20
        or 0x7F <= codepoint <= 0x9F
        or codepoint == 0x00AD
        or 0x0600 <= codepoint <= 0x0605
        or codepoint in {0x061C, 0x06DD, 0x070F, 0x08E2, 0x180E}
        or 0x0890 <= codepoint <= 0x0891
        or 0x200B <= codepoint <= 0x200F
        or 0x2028 <= codepoint <= 0x202E
        or 0x2060 <= codepoint <= 0x206F
        or 0xD800 <= codepoint <= 0xDFFF
        or codepoint == 0xFEFF
        or 0xFFF9 <= codepoint <= 0xFFFB
        or codepoint in {0x110BD, 0x110CD, 0xE0001}
        or 0x13430 <= codepoint <= 0x1343F
        or 0x1BCA0 <= codepoint <= 0x1BCA3
        or 0x1D173 <= codepoint <= 0x1D17A
        or 0xE0020 <= codepoint <= 0xE007F
    )


def readable_message(item: dict, user_id: int) -> dict | None:
    sender = item.get("from")
    body = item.get("body")
    timestamp_ms = item.get("time")
    if not isinstance(sender, dict) or not isinstance(body, dict):
        return None
    sender_id = sender.get("uid")
    text = body.get("text")
    if (
        type(sender_id) is not int
        or sender_id <= 0
        or not isinstance(text, str)
        or not text
        or any(unsafe_history_character(character) for character in text)
        or type(timestamp_ms) is not int
        or timestamp_ms <= 0
        or timestamp_ms > 2**63 - 1
    ):
        return None
    return {
        "direction": "outgoing" if sender_id == user_id else "incoming",
        "text": text,
        "timestamp_ms": timestamp_ms,
    }


def bounded_history_response(updated_cookie: str, readable: list[dict]) -> dict:
    bounded = list(readable)
    while True:
        response = {
            "ok": True,
            "action": "history",
            "verification": "exact_encrypt_job_id_and_user_id",
            "count": len(bounded),
            "messages": bounded,
            "updated_cookie": updated_cookie,
        }
        encoded = json.dumps(
            response, ensure_ascii=False, separators=(",", ":")
        ).encode("utf-8")
        if len(encoded) + 1 <= MAX_HISTORY_RESPONSE_BYTES:
            return response
        if not bounded:
            raise SafeFailure("Zhipin chat history result exceeded the safe output budget")
        bounded.pop(0)


def readable_history(
    cookie: str, remote_id: str, limit: int
) -> dict:
    deadline = time.monotonic() + SEND_DEADLINE_SECONDS
    if type(limit) is not int or not 1 <= limit <= MAX_HISTORY_MESSAGES:
        raise SafeFailure("chat history limit must be between 1 and 20")

    session, pairs, _ = prepare_session(cookie, deadline)
    friend = exact_friend(session, remote_id, deadline)
    if friend is None:
        raise SafeFailure("chat history requires an existing exact Zhipin conversation")
    boss_id = bounded_string(friend.get("encryptBossId"), "encrypted boss id")

    user_payload = request_json(
        session.get(
            USER_INFO_URL,
            headers=HEADERS,
            timeout=request_timeout(deadline),
        ),
        "Zhipin user info",
    )
    user_data = response_data(user_payload, "Zhipin user info")
    user_id = positive_int(user_data.get("userId"), "user id")

    readable: list[dict] = []
    for item in history_messages(session, boss_id, deadline):
        message = readable_message(item, user_id)
        if message is None or len(message["text"]) > MAX_HISTORY_TEXT_CHARS:
            continue
        readable.append(message)

    readable.sort(key=lambda item: item["timestamp_ms"])
    readable = readable[-limit:]
    return bounded_history_response(cookie_header(pairs), readable)


def bounded_inbox_response(updated_cookie: str, conversations: list[dict]) -> dict:
    response = {
        "ok": True,
        "action": "inbox",
        "verification": "exact_encrypt_job_ids_and_user_id",
        "count": len(conversations),
        "conversations": conversations,
        "updated_cookie": updated_cookie,
    }
    encoded = json.dumps(
        response, ensure_ascii=False, separators=(",", ":")
    ).encode("utf-8")
    if len(encoded) + 1 > MAX_INBOX_RESPONSE_BYTES:
        raise SafeFailure("Zhipin chat inbox result exceeded the safe output budget")
    return response


def readable_inbox(cookie: str, remote_ids: list[str]) -> dict:
    deadline = time.monotonic() + SEND_DEADLINE_SECONDS
    remote_ids = validate_inbox_remote_ids(remote_ids)
    session, pairs, _ = prepare_session(cookie, deadline)
    friends = exact_friends(session, remote_ids, deadline)

    user_payload = request_json(
        session.get(
            USER_INFO_URL,
            headers=HEADERS,
            timeout=request_timeout(deadline),
        ),
        "Zhipin user info",
    )
    user_data = response_data(user_payload, "Zhipin user info")
    user_id = positive_int(user_data.get("userId"), "user id")

    conversations: list[dict] = []
    for remote_id, friend in zip(remote_ids, friends, strict=True):
        boss_id = bounded_string(friend.get("encryptBossId"), "encrypted boss id")
        readable = [
            message
            for item in recent_history_page(session, boss_id, deadline)
            if (message := readable_message(item, user_id)) is not None
        ]
        latest = (
            max(readable, key=lambda item: item["timestamp_ms"])
            if readable
            else None
        )
        if latest is not None:
            text = latest["text"]
            latest = {
                "direction": latest["direction"],
                "text": text[:MAX_INBOX_TEXT_CHARS],
                "timestamp_ms": latest["timestamp_ms"],
                "truncated": len(text) > MAX_INBOX_TEXT_CHARS,
            }
        conversations.append({"remote_id": remote_id, "latest": latest})

    return bounded_inbox_response(cookie_header(pairs), conversations)


def encode_varint(value: int) -> bytes:
    if type(value) is not int or value < 0 or value > 2**64 - 1:
        raise SafeFailure("protobuf unsigned integer is invalid")
    encoded = bytearray()
    while value >= 0x80:
        encoded.append((value & 0x7F) | 0x80)
        value >>= 7
    encoded.append(value)
    return bytes(encoded)


def encode_varint_field(field: int, value: int) -> bytes:
    if type(field) is not int or field <= 0 or field > 536_870_911:
        raise SafeFailure("protobuf field number is invalid")
    return encode_varint(field << 3) + encode_varint(value)


def encode_bytes_field(field: int, value: bytes) -> bytes:
    if type(field) is not int or field <= 0 or field > 536_870_911:
        raise SafeFailure("protobuf field number is invalid")
    if not isinstance(value, bytes) or len(value) > 64 * 1024:
        raise SafeFailure("protobuf field payload is invalid")
    return encode_varint((field << 3) | 2) + encode_varint(len(value)) + value


def encode_text_field(field: int, value: str) -> bytes:
    return encode_bytes_field(field, value.encode("utf-8"))


def encode_user(uid: int, name: str | None, source: int) -> bytes:
    payload = encode_varint_field(1, uid)
    if name is not None:
        payload += encode_text_field(2, name)
    payload += encode_varint_field(7, source)
    return payload


def encode_body(message: str) -> bytes:
    return (
        encode_varint_field(1, 1)
        + encode_varint_field(2, 1)
        + encode_text_field(3, message)
    )


def encode_protocol(
    user_id: int,
    target_uid: int,
    target_name: str,
    target_source: int,
    message: str,
    timestamp_ms: int,
    message_id: int,
) -> bytes:
    chat_message = (
        encode_bytes_field(1, encode_user(user_id, None, 0))
        + encode_bytes_field(
            2, encode_user(target_uid, target_name, target_source)
        )
        + encode_varint_field(3, 1)
        + encode_varint_field(4, message_id)
        + encode_varint_field(5, timestamp_ms)
        + encode_bytes_field(6, encode_body(message))
        + encode_varint_field(11, message_id)
        + encode_varint_field(20, 1)
    )
    return encode_varint_field(1, 1) + encode_bytes_field(3, chat_message)


def publish_once(
    payload: bytes,
    cookie: str,
    token: str,
    wt2: str,
    deadline: float,
) -> None:
    import paho.mqtt.client as mqtt

    connection_finished = Event()
    connection_result = {"accepted": False, "reason": "unknown"}

    def on_connect(
        client: mqtt.Client,
        userdata: object,
        flags: mqtt.ConnectFlags,
        reason_code: mqtt.ReasonCode,
        properties: mqtt.Properties | None,
    ) -> None:
        del client, userdata, flags, properties
        if reason_code == 0:
            connection_result["accepted"] = True
            connection_result["reason"] = "success"
        else:
            connection_result["reason"] = str(reason_code)
        connection_finished.set()

    client = mqtt.Client(
        callback_api_version=mqtt.CallbackAPIVersion.VERSION2,
        client_id=f"ws-{secrets.token_hex(8)}",
        clean_session=True,
        protocol=mqtt.MQTTv311,
        transport="websockets",
        reconnect_on_failure=False,
    )
    client.on_connect = on_connect
    client.username_pw_set(f"{token}|0", wt2)
    client.tls_set(cert_reqs=ssl.CERT_REQUIRED)
    client.ws_set_options(
        path="/chatws",
        headers={
            "Cookie": cookie,
            "Origin": BASE_URL,
            "User-Agent": USER_AGENT,
            "Sec-WebSocket-Protocol": wt2,
        },
    )
    client.max_inflight_messages_set(1)
    started = False
    try:
        if client.connect("ws.zhipin.com", 443, keepalive=30) != mqtt.MQTT_ERR_SUCCESS:
            raise SafeFailure("Zhipin chat connection failed")
        client.loop_start()
        started = True
        wait = request_timeout(deadline)
        if not connection_finished.wait(wait):
            raise SafeFailure("Zhipin chat connection timed out before acknowledgement")
        if not connection_result["accepted"]:
            raise SafeFailure(
                f"Zhipin chat connection was rejected: {connection_result['reason']}"
            )
        published = client.publish("chat", payload, qos=1, retain=False)
        if published.rc != mqtt.MQTT_ERR_SUCCESS:
            raise SafeFailure("Zhipin chat publish failed")
        published.wait_for_publish(timeout=request_timeout(deadline))
    finally:
        if started:
            client.disconnect()
            client.loop_stop()


def send(cookie: str, remote_id: str, message: str) -> dict:
    deadline = time.monotonic() + SEND_DEADLINE_SECONDS
    message = normalize_message(message)
    session, pairs, _ = prepare_session(cookie, deadline)
    friend = exact_friend(session, remote_id, deadline)
    if friend is None:
        raise SafeFailure("chat send requires an existing exact Zhipin conversation")

    target_uid = positive_int(friend.get("uid"), "conversation uid")
    target_name = bounded_string(friend.get("encryptBossId"), "encrypted boss id")
    target_source = exact_int(friend.get("friendSource"), "conversation source")

    user_payload = request_json(
        session.get(
            USER_INFO_URL,
            headers=HEADERS,
            timeout=request_timeout(deadline),
        ),
        "Zhipin user info",
    )
    user_data = response_data(user_payload, "Zhipin user info")
    user_id = positive_int(user_data.get("userId"), "user id")
    token = bounded_string(user_data.get("token"), "user token")

    wt_payload = request_json(
        session.get(
            WT_URL,
            headers=HEADERS,
            timeout=request_timeout(deadline),
        ),
        "Zhipin websocket credential",
    )
    wt_data = response_data(wt_payload, "Zhipin websocket credential")
    wt2 = bounded_string(wt_data.get("wt2"), "websocket credential")

    before = history_messages(session, target_name, deadline)
    if has_exact_outgoing_text(before, user_id, message):
        return {
            "ok": True,
            "action": "send",
            "state": "already_sent",
            "verification": "exact_outgoing_text_in_history",
            "updated_cookie": cookie_header(pairs),
        }

    timestamp_ms = int(time.time() * 1000)
    message_id = user_id + timestamp_ms
    if message_id > 2**63 - 1:
        raise SafeFailure("Zhipin message identifier is invalid")
    payload = encode_protocol(
        user_id,
        target_uid,
        target_name,
        target_source,
        message,
        timestamp_ms,
        message_id,
    )
    publish_once(payload, cookie_header(pairs), token, wt2, deadline)

    for attempt in range(3):
        after = history_messages(session, target_name, deadline)
        if has_exact_outgoing_text(after, user_id, message):
            return {
                "ok": True,
                "action": "send",
                "state": "message_verified",
                "verification": "exact_outgoing_text_in_history",
                "updated_cookie": cookie_header(pairs),
            }
        if attempt < 2:
            time.sleep(min(0.5, max(0.0, deadline - time.monotonic())))
    raise SafeFailure("Zhipin message could not be verified")


def main() -> int:
    try:
        request = json.load(sys.stdin)
        if not isinstance(request, dict):
            raise SafeFailure("transport request must be an object")
        action = request.get("action")
        if action not in {"refresh", "greet", "send", "history", "inbox"}:
            raise SafeFailure("unsupported transport action")
        cookie = request.get("cookie")
        if not isinstance(cookie, str) or not cookie:
            raise SafeFailure("Zhipin Cookie is required")
        if action == "refresh":
            if set(request) != {"action", "cookie"}:
                raise SafeFailure("transport request contains unsupported fields")
            response = refresh(cookie)
        elif action == "greet":
            if set(request) != {"action", "cookie", "title", "remote_id"}:
                raise SafeFailure("transport request contains unsupported fields")
            title = request.get("title")
            remote_id = request.get("remote_id")
            if not isinstance(title, str) or not title.strip():
                raise SafeFailure("cached Zhipin job title is required")
            if not isinstance(remote_id, str) or not remote_id:
                raise SafeFailure("cached Zhipin job identifier is required")
            response = greet(cookie, title.strip(), remote_id)
        elif action == "send":
            if set(request) != {"action", "cookie", "remote_id", "message"}:
                raise SafeFailure("transport request contains unsupported fields")
            remote_id = request.get("remote_id")
            message = request.get("message")
            if not isinstance(remote_id, str) or not remote_id:
                raise SafeFailure("cached Zhipin job identifier is required")
            if not isinstance(message, str):
                raise SafeFailure("chat message must be text")
            response = send(cookie, remote_id, message)
        elif action == "history":
            if set(request) != {"action", "cookie", "remote_id", "limit"}:
                raise SafeFailure("transport request contains unsupported fields")
            remote_id = request.get("remote_id")
            limit = request.get("limit")
            if not isinstance(remote_id, str) or not remote_id:
                raise SafeFailure("cached Zhipin job identifier is required")
            response = readable_history(cookie, remote_id, limit)
        else:
            if set(request) != {"action", "cookie", "remote_ids"}:
                raise SafeFailure("transport request contains unsupported fields")
            response = readable_inbox(
                cookie,
                validate_inbox_remote_ids(request.get("remote_ids")),
            )
    except SafeFailure as error:
        response = {"ok": False, "error": str(error)}
    except Exception:
        response = {"ok": False, "error": "Zhipin transport failed safely"}
    json.dump(response, sys.stdout, ensure_ascii=False, separators=(",", ":"))
    sys.stdout.write("\n")
    return 0 if response.get("ok") else 1


if __name__ == "__main__":
    raise SystemExit(main())
