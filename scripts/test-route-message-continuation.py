#!/usr/bin/env python3
"""Drive the built route_message guest against an in-memory OData host.

Usage: python3 scripts/test-route-message-continuation.py path/to/route_message.wasm
Requires wasmtime==38.0.0. No server or provider credentials are used.
"""

import json
from pathlib import Path
import re
import sys
import unittest
from urllib.parse import unquote, urlsplit

import wasmtime


FIRST_MESSAGE = "What changed in last night's deploy?"
FOLLOW_UP = "Please check again."
SESSION_REF = "session-entries:ss-1"
FIRST_LEAF = "u-ss-1-0"


class ChannelHost:
    def __init__(self, fields, entries=()):
        self.prior = {"entity_id": "ss-1", "fields": fields}
        self.entries = {entry["EntryId"]: entry.copy() for entry in entries}
        self.entry_writes = []
        self.configured = None
        self.binding = None
        self.logs = []
        self.result = None

    def request(self, method, url, headers, body):
        path = unquote(urlsplit(url).path)
        if method == "GET" and path == "/tdata/ChannelSessions":
            return {"value": [{"entity_id": "cs-1", "SessionEntityId": "ss-1"}]}
        if method == "GET" and path == "/tdata/Sessions('ss-1')":
            return self.prior
        if method == "GET" and path.startswith("/tdata/SessionEntries("):
            match = re.fullmatch(
                r"/tdata/SessionEntries\(SessionId='ss-1',EntryId='([^']+)'\)", path
            )
            assert match, path
            entry = self.entries.get(match[1])
            return {"value": [entry] if entry else []}
        if method == "POST" and path == "/tdata/SessionEntries":
            entry = json.loads(body)
            assert entry["SessionId"] == "ss-1", entry
            # Keep every write, including duplicates, so reuse is observable.
            self.entry_writes.append(entry)
            self.entries[entry["EntryId"]] = entry
            return dict(entry, entity_id="se-" + entry["EntryId"])
        if method == "POST" and path == "/tdata/Sessions":
            return {"entity_id": "ss-2"}
        if method == "POST" and path == "/tdata/Sessions('ss-2')/TemperPaw.Configure":
            self.configured = json.loads(body)
            return {}
        if method == "POST" and path == "/tdata/ChannelSessions('cs-1')/Paw.Channel.UpdateSession":
            self.binding = json.loads(body)
            return {}
        raise AssertionError(f"Unexpected HTTP request: {method} {url} {body}")

    @staticmethod
    def read(caller, pointer, size):
        return bytes(caller.get("memory").read(caller, pointer, pointer + size)).decode()

    @staticmethod
    def write(caller, pointer, capacity, value):
        data = value.encode()
        assert len(data) <= capacity
        caller.get("memory").write(caller, data, pointer)
        return len(data)

    def run(self, engine, module):
        context = json.dumps({
            "integration_config": {"temper_api_url": "http://temper.test"},
            "tenant": "test",
            "entity_type": "Channel",
            "entity_id": "ch-1",
            "trigger_action": "ReceiveMessage",
            "entity_state": {"fields": {
                "channel_id": "channel-1", "thread_id": "thread-1",
                "author_id": "person-1", "content": FOLLOW_UP,
            }},
        })

        def http(caller, mp, ml, up, ul, hp, hl, bp, bl, out, capacity):
            response = self.request(
                self.read(caller, mp, ml), self.read(caller, up, ul),
                self.read(caller, hp, hl), self.read(caller, bp, bl),
            )
            return self.write(caller, out, capacity, "200\n" + json.dumps(response))

        def batch(caller, pointer, size, out, capacity):
            requests = json.loads(self.read(caller, pointer, size))
            responses = [
                {"status": 200, "body": json.dumps(self.request(**request))}
                for request in requests
            ]
            return self.write(caller, out, capacity, json.dumps(responses))

        def log(caller, lp, ll, mp, ml):
            self.logs.append(self.read(caller, mp, ml))

        def result(caller, pointer, size):
            self.result = json.loads(self.read(caller, pointer, size))

        callbacks = {
            "host_get_context": lambda caller, p, n: self.write(caller, p, n, context),
            "host_http_call": http,
            "host_http_call_batch": batch,
            "host_log": log,
            "host_set_result": result,
            "host_get_time_millis": lambda caller: 1_793_145_600_000,
        }
        store = wasmtime.Store(engine)
        store.set_wasi(wasmtime.WasiConfig())
        linker = wasmtime.Linker(engine)
        linker.define_wasi()
        for imp in module.imports:
            if imp.module == "env":
                linker.define_func(
                    imp.module, imp.name, imp.type, callbacks[imp.name], access_caller=True
                )
        instance = linker.instantiate(store, module)
        instance.exports(store)["run"](store, 0, 0)


def prior_fields(odata=False):
    fields = {
        "status": "Failed", "user_message": FIRST_MESSAGE,
        "session_entries_materialized": "false", "session_file_id": SESSION_REF,
        "session_leaf_id": FIRST_LEAF, "model": "test-model", "provider": "test-provider",
        "error": "provider login expired",
    }
    if odata:
        return {"".join(word.title() for word in key.split("_")): value
                for key, value in fields.items()}
    return fields


class ContinuationTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.engine = wasmtime.Engine()
        cls.module = wasmtime.Module.from_file(cls.engine, WASM_PATH)

    def assert_routed(self, host):
        self.assertTrue(host.result["success"], host.result)
        self.assertEqual(host.binding["session_entity_id"], "ss-2")
        self.assertEqual(host.configured["parent_session_id"], "ss-1")

    def test_continuation_saves_original_message_and_reuses_existing_first_turn(self):
        for odata in (False, True):
            with self.subTest(odata=odata):
                host = ChannelHost(prior_fields(odata))
                host.run(self.engine, self.module)
                self.assert_routed(host)
                self.assertEqual(len(host.entry_writes), 3)
                header, original, follow_up = sorted(host.entries.values(), key=lambda e: e["Sequence"])
                self.assertEqual(header["EntryId"], "h-ss-1")
                self.assertEqual(header["EntryType"], "header")
                self.assertEqual(header["ParentEntryId"], "")
                self.assertEqual(original["EntryId"], FIRST_LEAF)
                self.assertEqual(original["ParentEntryId"], header["EntryId"])
                self.assertEqual(original["Role"], "user")
                self.assertEqual(json.loads(original["Content"]), FIRST_MESSAGE)
                self.assertEqual(follow_up["ParentEntryId"], original["EntryId"])
                self.assertEqual(follow_up["Sequence"], original["Sequence"] + 1)
                self.assertEqual(follow_up["Role"], "user")
                self.assertEqual(json.loads(follow_up["Content"]), FOLLOW_UP)
                self.assertEqual(host.configured["session_file_id"], SESSION_REF)
                self.assertEqual(host.configured["session_leaf_id"], follow_up["EntryId"])
                self.assertTrue(any("wrote the unwritten first turn" in log for log in host.logs))

                # The entries landed, but the failed Session's materialized flag
                # still says false. A retry must reuse them without initial writes.
                retry = ChannelHost(prior_fields(odata), [header, original])
                retry.run(self.engine, self.module)
                self.assert_routed(retry)
                self.assertEqual(retry.entry_writes, [follow_up])
                self.assertEqual(retry.entries[FIRST_LEAF], original)
                self.assertEqual(retry.configured["session_file_id"], SESSION_REF)
                self.assertEqual(retry.configured["session_leaf_id"], follow_up["EntryId"])
                self.assertFalse(any("wrote the unwritten first turn" in log for log in retry.logs))

    def test_other_missing_leaves_still_start_fresh(self):
        for overrides in ({"session_entries_materialized": "true"}, {"session_leaf_id": "a-3"}):
            with self.subTest(overrides=overrides):
                host = ChannelHost(dict(prior_fields(), **overrides))
                host.run(self.engine, self.module)
                self.assert_routed(host)
                self.assertEqual(host.entry_writes, [])
                self.assertEqual(host.configured["session_file_id"], "")
                self.assertEqual(host.configured["session_leaf_id"], "")
                self.assertIn(FOLLOW_UP, host.configured["user_message"])
                self.assertTrue(any("starting clean continuation" in log for log in host.logs))


if __name__ == "__main__":
    WASM_PATH = Path(sys.argv.pop(1))
    unittest.main()
