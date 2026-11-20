"""Run against the optional web Compose service and real DataVault HTTP endpoints."""
import http.cookiejar
import json
import urllib.error
import urllib.parse
import urllib.request
import uuid

import pytest

from client import account, admin, s3

WEB = "http://web:8080"


class Browser:
    def __init__(self):
        self.cookies = http.cookiejar.CookieJar()
        self.opener = urllib.request.build_opener(urllib.request.HTTPCookieProcessor(self.cookies))

    def request(self, method, path, *, data=None, body=None, origin=WEB):
        headers = {"X-DataVault-Request": "1", "Origin": origin}
        if data is not None:
            body = json.dumps(data).encode()
            headers["Content-Type"] = "application/json"
        elif body is not None:
            headers["Content-Type"] = "application/octet-stream"
        req = urllib.request.Request(WEB + path, method=method, headers=headers, data=body)
        with self.opener.open(req, timeout=20) as response:
            return response.status, response.headers, response.read()

    def json(self, method, path, **kwargs):
        return json.loads(self.request(method, path, **kwargs)[2])


def test_web_gateway_access_and_transfers():
    browser = Browser()
    assert browser.json("GET", "/web-api/health") == {"ready": True}
    with pytest.raises(urllib.error.HTTPError) as anonymous:
        browser.request("GET", "/web-api/buckets")
    assert anonymous.value.code == 401

    uid, credential = account(True)
    name = "web-" + uuid.uuid4().hex
    key = "reports/Q4 + inventory #1.bin"
    query = urllib.parse.urlencode({"key": key})
    base = f"/web-api/buckets/{name}"
    owner = s3(credential)
    created = False
    try:
        session = browser.json("POST", "/web-api/session", data=credential)
        assert session["account"].endswith(credential["access_key"][-4:])
        assert credential["secret_key"] not in str(session)
        assert any(cookie.has_nonstandard_attr("HttpOnly") for cookie in browser.cookies)
        with pytest.raises(urllib.error.HTTPError) as cross_origin:
            browser.request("POST", "/web-api/buckets", data={"name": name}, origin="http://different.invalid")
        assert cross_origin.value.code == 403
        assert browser.json("POST", "/web-api/buckets", data={"name": name})["name"] == name
        created = True
        assert name in browser.json("GET", "/web-api/buckets")["buckets"]

        data = bytes(range(256)) * 256
        assert browser.json("PUT", base + "/object?" + query, body=data)["key"] == key
        with pytest.raises(urllib.error.HTTPError) as conflict:
            browser.request("PUT", base + "/object?" + query, body=b"replacement")
        assert conflict.value.code == 409
        assert browser.json("GET", base + "/objects")["folders"] == ["reports/"]
        assert browser.json("GET", base + "/objects?prefix=reports%2FQ4&flat=1")["objects"][0]["key"] == key
        status, headers, downloaded = browser.request("GET", base + "/download?" + query)
        assert status == 200 and downloaded == data
        assert headers["Content-Disposition"].startswith("attachment;")
        assert int(browser.request("HEAD", base + "/download?" + query)[1]["Content-Length"]) == len(data)
        browser.request("PUT", base + "/object?" + query + "&replace=1", body=b"")
        assert owner.get_object(Bucket=name, Key=key)["Body"].read() == b""

        reader_id, reader_key = account()
        try:
            admin("POST", "permissions", {"account_id": reader_id, "bucket": name, "role": "ReadOnly"})
            reader = Browser()
            reader.json("POST", "/web-api/session", data=reader_key)
            assert reader.request("GET", base + "/download?" + query)[2] == b""
            with pytest.raises(urllib.error.HTTPError) as denied:
                reader.request("PUT", base + "/object?" + query + "&replace=1", body=b"denied")
            assert denied.value.code == 403
        finally:
            admin("DELETE", "service-accounts/" + reader_id)
        browser.request("DELETE", "/web-api/session")
        assert browser.json("GET", "/web-api/session")["account"] is None
        with pytest.raises(urllib.error.HTTPError) as ended:
            browser.request("GET", "/web-api/buckets")
        assert ended.value.code == 401
    finally:
        if created:
            owner.delete_object(Bucket=name, Key=key)
            owner.delete_bucket(Bucket=name)
        admin("DELETE", "service-accounts/" + uid)
