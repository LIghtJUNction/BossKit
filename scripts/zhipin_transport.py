#!/usr/bin/env python3
"""Browserless BOSS Zhipin session transport used by the Rust CLI.

The process reads one JSON request from stdin and writes one JSON response to
stdout. Credential values are returned only to the parent process and are
never included in diagnostics.
"""

from __future__ import annotations

import json
import os
import sys
from http.cookies import SimpleCookie
from urllib.parse import quote

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

FRIEND_LIST_URL = (
    "https://www.zhipin.com/wapi/zprelation/friend/getGeekFriendList.json"
)
FRIEND_ADD_URL = "https://www.zhipin.com/wapi/zpgeek/friend/add.json"
API_URL = "https://www.zhipin.com/wapi/zpgeek/search/joblist.json"
BASE_URL = "https://www.zhipin.com"
MAX_LOOKUP_PAGES = 3
LOOKUP_PAGE_SIZE = 30
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


def challenge_token(seed: str, name: str, timestamp: int) -> str:
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
        timeout=15,
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
) -> list[tuple[str, str]]:
    challenge = payload.get("zpData")
    if not isinstance(challenge, dict):
        raise SafeFailure("Zhipin security challenge is missing")
    seed = challenge.get("seed")
    name = challenge.get("name")
    timestamp = challenge.get("ts")
    if not isinstance(seed, str) or not isinstance(name, str):
        raise SafeFailure("Zhipin security challenge is incomplete")
    token = challenge_token(seed, name, timestamp)
    pairs = replace_cookie(pairs, "__zp_stoken__", token)
    session.cookies.set(
        "__zp_stoken__",
        token,
        domain=".zhipin.com",
        path="/",
    )
    return pairs


def friend_list(session: requests.Session, page: int) -> dict:
    return request_json(
        session.get(
            FRIEND_LIST_URL,
            headers=HEADERS,
            params={"page": str(page)},
            timeout=15,
        ),
        "Zhipin friend list",
    )


def prepare_session(
    cookie: str,
) -> tuple[requests.Session, list[tuple[str, str]], bool]:
    pairs = cookie_pairs(cookie)
    session = requests.Session()
    for name, value in pairs:
        session.cookies.set(name, value, domain=".zhipin.com", path="/")

    initial = request_json(
        session.post(
            API_URL,
            headers=HEADERS,
            data=SEARCH_DATA,
            timeout=15,
        ),
        "Zhipin session check",
    )
    code = api_code(initial, "Zhipin session check")
    token_refreshed = False
    if code == 37:
        pairs = apply_challenge(initial, pairs, session)
        token_refreshed = True
        verified = request_json(
            session.post(
                API_URL,
                headers=HEADERS,
                data=SEARCH_DATA,
                timeout=15,
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

    authenticated = friend_list(session, 1)
    authenticated_code = api_code(authenticated, "Zhipin authenticated session check")
    if authenticated_code == 37:
        pairs = apply_challenge(authenticated, pairs, session)
        token_refreshed = True
        authenticated = friend_list(session, 1)
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


def has_exact_friend(session: requests.Session, remote_id: str) -> bool:
    for page in range(1, MAX_LOOKUP_PAGES + 1):
        payload = friend_list(session, page)
        code = api_code(payload, "Zhipin friend list")
        if code != 0:
            raise SafeFailure(f"Zhipin friend list failed with API code {code!r}")
        items = result_items(payload, "Zhipin friend list")
        if any(friend_job_id(item) == remote_id for item in items):
            return True
        if not items:
            return False
    return False


def search_exact_job(
    session: requests.Session, title: str, remote_id: str
) -> tuple[str, str]:
    matches: list[dict] = []
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
    return security_id, lid


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

    security_id, lid = search_exact_job(session, title, remote_id)
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


def main() -> int:
    try:
        request = json.load(sys.stdin)
        if not isinstance(request, dict):
            raise SafeFailure("transport request must be an object")
        action = request.get("action")
        if action not in {"refresh", "greet"}:
            raise SafeFailure("unsupported transport action")
        cookie = request.get("cookie")
        if not isinstance(cookie, str) or not cookie:
            raise SafeFailure("Zhipin Cookie is required")
        if action == "refresh":
            if set(request) != {"action", "cookie"}:
                raise SafeFailure("transport request contains unsupported fields")
            response = refresh(cookie)
        else:
            if set(request) != {"action", "cookie", "title", "remote_id"}:
                raise SafeFailure("transport request contains unsupported fields")
            title = request.get("title")
            remote_id = request.get("remote_id")
            if not isinstance(title, str) or not title.strip():
                raise SafeFailure("cached Zhipin job title is required")
            if not isinstance(remote_id, str) or not remote_id:
                raise SafeFailure("cached Zhipin job identifier is required")
            response = greet(cookie, title.strip(), remote_id)
    except SafeFailure as error:
        response = {"ok": False, "error": str(error)}
    except Exception:
        response = {"ok": False, "error": "Zhipin transport failed safely"}
    json.dump(response, sys.stdout, ensure_ascii=False, separators=(",", ":"))
    sys.stdout.write("\n")
    return 0 if response.get("ok") else 1


if __name__ == "__main__":
    raise SystemExit(main())
