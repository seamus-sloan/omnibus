"""Unit tests for the intent-vs-state audit.

Run them with `python3 -m unittest discover -s scripts/explore -p 'tests.py'`
or via `just explore-test`, which `just test` depends on. Stdlib `unittest`
on purpose: the
exploration scripts assume nothing beyond the system Python `lib.sh` already
relies on, and no Nix shell here carries pytest.

The state-dependent tests drive a fake `ActorState` rather than the instance,
so the comparison rules — which are where a false positive comes from — are
covered without a live server.
"""

from __future__ import annotations

import json
import multiprocessing
import tempfile
import unittest
from pathlib import Path
from typing import Any

from . import compare, env, expectations, journal, replay, vocabulary
from .client import ApiError, load_accounts


def entry(action: str, **kw: Any) -> journal.Entry:
    obj = {
        "ts": "2026-08-28T15:00:00.000Z",
        "run": "r-20260828-02",
        "actor": kw.pop("actor", "agent-1"),
        "surface": "web",
        "flow": "test",
        "seq": kw.pop("seq", 1),
        "action": action,
        "target": kw.pop("target", None),
        "params": kw.pop("params", {}),
        "outcome": kw.pop("outcome", "ok"),
        "note": None,
    }
    obj.update(kw)
    return journal.Entry.from_obj(obj)


class FakeState:
    """Enough of `ActorState` for the comparison rules."""

    def __init__(self, client: Any = None, **facts: Any) -> None:
        self.facts = facts
        self.client = client

    def rating(self, uuid: str) -> Any:
        return self.facts.get("ratings", {}).get(uuid)

    def read_status(self, uuid: str) -> Any:
        return self.facts.get("statuses", {}).get(uuid)

    def progress(self, uuid: str, axis: str = "ebook") -> Any:
        return self.facts.get("progress", {}).get((uuid, axis))

    def playback_rate(self, uuid: str) -> Any:
        return self.facts.get("rates", {}).get(uuid)

    def journals(self, uuid: str) -> list[dict[str, Any]]:
        return self.facts.get("journals", {}).get(uuid, [])

    def highlights(self, uuid: str) -> list[dict[str, Any]]:
        return self.facts.get("highlights", {}).get(uuid, [])

    def bookmarks(self, uuid: str) -> list[dict[str, Any]]:
        return self.facts.get("bookmarks", {}).get(uuid, [])

    def shelves(self) -> list[dict[str, Any]]:
        return self.facts.get("shelves", [])

    def shelf_members(self, shelf_id: int) -> list[str]:
        return self.facts.get("members", {}).get(shelf_id, [])

    def wishlist(self) -> list[str]:
        return self.facts.get("wishlist", [])

    def library(self) -> list[str]:
        return self.facts.get("library", [])


def claims_of(*slots: tuple, shelf_names: Any = (), shelf_any: bool = False) -> expectations.Claims:
    c = expectations.Claims()
    c.slots.update(slots)
    c.shelf_names.update(shelf_names)
    c.shelf_any = shelf_any
    return c


class FakeClient:
    """Records what replay sends and answers from a canned table."""

    def __init__(self, responses: dict | None = None) -> None:
        self.calls: list[tuple[str, str, Any]] = []
        self.responses = responses or {}

    def _answer(self, method: str, path: str, payload: Any) -> tuple[int, str]:
        self.calls.append((method, path, payload))
        return self.responses.get(path, (200, "{}"))

    def post(self, path: str, payload: Any) -> tuple[int, str]:
        return self._answer("POST", path, payload)

    def put(self, path: str, payload: Any) -> tuple[int, str]:
        return self._answer("PUT", path, payload)

    def patch(self, path: str, payload: Any) -> tuple[int, str]:
        return self._answer("PATCH", path, payload)


BOOK = "18c784fc-e768-47c5-9d6c-ceb3e0cbb3db"


class VocabularyTests(unittest.TestCase):
    def test_classify_recognises_a_documented_write(self) -> None:
        cls = vocabulary.classify("rating.set")
        self.assertTrue(cls.is_write)
        self.assertEqual(cls.family, "rating")

    def test_classify_folds_plural_and_separator_spellings_onto_one_noun(self) -> None:
        self.assertEqual(vocabulary.classify("ratings.set").family, "rating")
        self.assertEqual(vocabulary.classify("shelves.create").family, "shelf")
        self.assertEqual(vocabulary.classify("playback.rate").family, "playback_rate")

    def test_classify_reads_a_trailing_qualifier_as_an_observation(self) -> None:
        for name in ("book.add.verify", "reader.resume_check", "reader.resume.verify", "wishlist.verify"):
            self.assertEqual(vocabulary.classify(name).kind, vocabulary.OBSERVATION, name)

    def test_classify_reads_a_trailing_qualifier_behind_filler_as_an_observation(self) -> None:
        self.assertEqual(vocabulary.classify("journal.persist.verify").kind, vocabulary.OBSERVATION)

    def test_classify_marks_metadata_out_of_scope_with_the_contract_reason(self) -> None:
        cls = vocabulary.classify("metadata.save")
        self.assertEqual(cls.kind, vocabulary.OUT_OF_SCOPE)
        self.assertEqual(cls.detail, vocabulary.SCOPE_METADATA)

    def test_classify_never_guesses_a_write_from_an_unknown_verb(self) -> None:
        for name in ("shelf.rename", "player.scrub", "journal.pin"):
            cls = vocabulary.classify(name)
            self.assertEqual(cls.kind, vocabulary.UNKNOWN, name)
            self.assertIn(name, cls.reason or "")

    def test_an_unlisted_verb_on_a_state_free_noun_is_a_look(self) -> None:
        # The verb slot is open exactly where the noun holds nothing to miss.
        for name in ("nav.jump", "search.refine", "stats.expand", "ui.hover"):
            self.assertEqual(vocabulary.classify(name).kind, vocabulary.OBSERVATION, name)

    def test_an_unlisted_verb_on_an_excluded_noun_stays_out_of_scope(self) -> None:
        cls = vocabulary.classify("metadata.revert")
        self.assertEqual(cls.kind, vocabulary.OUT_OF_SCOPE)
        self.assertEqual(cls.detail, vocabulary.SCOPE_METADATA)

    def test_classify_resolves_a_two_segment_noun(self) -> None:
        for name in ("read-status.set", "read_status.set", "readstatus.set"):
            self.assertEqual(vocabulary.classify(name).family, "read_status", name)

    def test_a_deep_name_resolves_through_a_non_write_verb_at_either_end(self) -> None:
        self.assertEqual(vocabulary.classify("book.detail.open").kind, vocabulary.OBSERVATION)
        self.assertEqual(vocabulary.classify("reader.settings.font_size").kind, vocabulary.OBSERVATION)

    def test_a_deep_name_never_collapses_onto_a_write_verb(self) -> None:
        # `shelf.archive` is not defined; reading `shelf.archive.all` as one
        # would assert a shelf that nothing created.
        self.assertEqual(vocabulary.classify("shelf.archive.all").kind, vocabulary.UNKNOWN)

    def test_confirm_is_the_act_not_a_check_that_it_stuck(self) -> None:
        # `merge.confirm` presses the button; treating it as a look would
        # drop it from `unverifiable` entirely.
        self.assertEqual(vocabulary.classify("merge.confirm").kind, vocabulary.OUT_OF_SCOPE)
        self.assertEqual(vocabulary.classify("merge.attempt").kind, vocabulary.OBSERVATION)

    def test_classify_returns_unknown_rather_than_raising_on_junk(self) -> None:
        self.assertEqual(vocabulary.classify(None).kind, vocabulary.UNKNOWN)
        self.assertEqual(vocabulary.classify("...").kind, vocabulary.UNKNOWN)

    def test_player_rate_is_playback_speed_not_a_star_rating(self) -> None:
        self.assertEqual(vocabulary.classify("player.rate").family, "playback_rate")


class ParserTests(unittest.TestCase):
    def test_parse_rating_reads_prose_and_explicit_clears(self) -> None:
        self.assertEqual(expectations.parse_rating("3.5 of 5"), 3.5)
        self.assertEqual(expectations.parse_rating(4), 4.0)
        self.assertIsNone(expectations.parse_rating(None))
        self.assertIsNone(expectations.parse_rating("cleared"))

    def test_parse_rating_rejects_a_boolean_standing_where_a_value_was_expected(self) -> None:
        self.assertIs(expectations.parse_rating(True), expectations.UNPARSED)

    def test_parse_status_normalises_the_ui_wording(self) -> None:
        self.assertEqual(expectations.parse_status("reading (In progress)"), "reading")
        self.assertEqual(expectations.parse_status("Finished just now"), "finished")
        self.assertEqual(expectations.parse_status("unread (Not started)"), "unread")
        self.assertIs(expectations.parse_status(True), expectations.UNPARSED)

    def test_parse_rate_reads_the_ui_suffix(self) -> None:
        self.assertEqual(expectations.parse_rate("1.20x"), 1.2)
        self.assertIs(expectations.parse_rate(9.0), expectations.UNPARSED)

    def test_parse_percent_reads_a_position_string(self) -> None:
        self.assertEqual(expectations.parse_percent("Ch 9 of 64, p. 3 of 22, 11%"), 11.0)

    def test_parse_status_does_not_read_did_not_finish_as_finished(self) -> None:
        for phrase in ("did not finish", "didn't finish it", "never finished"):
            self.assertNotEqual(expectations.parse_status(phrase), "finished", phrase)
        self.assertEqual(expectations.parse_status("Finished just now"), "finished")

    def test_parse_rating_refuses_a_value_off_the_half_star_grid(self) -> None:
        # The UI rates in 0.5 steps; 4.3 is a misread, not a rating, and
        # asserting it would mismatch against every honest half-star row.
        self.assertIs(expectations.parse_rating(4.3), expectations.UNPARSED)
        self.assertEqual(expectations.parse_rating(4.5), 4.5)

    def test_a_bare_close_with_no_position_is_declined_not_asserted(self) -> None:
        # A cover-only open-and-close may never write a position server-side.
        exps, unver, _, _ = expectations.expectations_for(
            "agent-1", [entry("reader.close", target=BOOK, params={"exit_via": "Back to book"})]
        )
        self.assertEqual(exps, [])
        self.assertEqual(len(unver), 1)
        self.assertIn("no position recorded", unver[0].why)


class ExpectationTests(unittest.TestCase):
    def test_scalar_family_keeps_only_the_last_statement(self) -> None:
        entries = [
            entry("rating.set", seq=1, target=BOOK, params={"new": 4.0}),
            entry("rating.set", seq=2, target=BOOK, params={"old": 4.0, "new": None}),
            entry("rating.set", seq=3, target=BOOK, params={"old": None, "new": 4.5}),
        ]
        exps, unver, _, _ = expectations.expectations_for("agent-1", entries)
        self.assertEqual(len(exps), 1)
        self.assertEqual(exps[0].value, 4.5)
        self.assertEqual(unver, [])

    def test_rating_prefers_the_terminal_value_over_a_transition_list(self) -> None:
        e = entry(
            "rating.set",
            target=BOOK,
            params={"sequence": [{"old": "none", "new": "4.5 of 5"}], "final_rating_left_behind": "3.5 of 5"},
        )
        exps, _, _, _ = expectations.expectations_for("agent-1", [e])
        self.assertEqual(exps[0].value, 3.5)

    def test_status_falls_past_a_key_holding_a_boolean(self) -> None:
        e = entry(
            "status.set",
            target=BOOK,
            params={"old": "finished", "new": "reading", "left_in_this_state": True},
        )
        exps, _, _, _ = expectations.expectations_for("agent-1", [e])
        self.assertEqual(exps[0].value, "reading")

    def test_status_reads_the_last_of_a_transition_list(self) -> None:
        e = entry(
            "status.set",
            target=BOOK,
            params={"transitions": [{"old": "reading", "new": "finished"}, {"old": "finished", "new": "reading"}]},
        )
        exps, _, _, _ = expectations.expectations_for("agent-1", [e])
        self.assertEqual(exps[0].value, "reading")

    def test_add_then_remove_expects_nothing(self) -> None:
        entries = [
            entry("wishlist.add", seq=1, target="a-uuid", params={"chosen_title": "Dune"}),
            entry("wishlist.remove", seq=2, target="a-uuid", params={}),
        ]
        exps, _, _, _ = expectations.expectations_for("agent-3", entries)
        self.assertEqual(exps, [])

    def test_a_remove_of_something_this_run_never_added_cancels_nothing(self) -> None:
        # The bug this guards: agent adds A, then removes B (added by an
        # earlier run). A pop that falls back to "drop the most recent" would
        # cancel A's expectation, and the audit would never look for it.
        entries = [
            entry("wishlist.add", seq=1, target="uuid-A", params={"chosen_title": "A"}),
            entry("wishlist.remove", seq=2, target="uuid-B", params={}),
        ]
        exps, unver, _, _ = expectations.expectations_for("agent-3", entries)
        self.assertEqual([e.value for e in exps], ["uuid-A"])
        self.assertIn("did not add", unver[0].why)

    def test_a_shelf_delete_this_run_never_created_cancels_nothing(self) -> None:
        entries = [
            entry("shelf.create", seq=1, params={"name": "mine"}),
            entry("shelf.delete", seq=2, params={"name": "someone else's"}),
        ]
        exps, unver, _, _ = expectations.expectations_for("agent-2", entries)
        self.assertEqual([e.value for e in exps], ["mine"])
        self.assertIn("did not create", unver[0].why)

    def test_journal_update_supersedes_its_create(self) -> None:
        entries = [
            entry("journal.create", seq=1, target=BOOK, params={"entry_text_verbatim": "first draft"}),
            entry("journal.update", seq=2, target=BOOK, params={"after_verbatim": "first draft plus more"}),
        ]
        exps, _, _, _ = expectations.expectations_for("agent-1", entries)
        self.assertEqual([e.value for e in exps], ["first draft plus more"])

    def test_a_write_with_no_readable_value_is_unverifiable_not_a_finding(self) -> None:
        e = entry("rating.set", target=BOOK, params={"how": "clicked the stars"})
        exps, unver, _, _ = expectations.expectations_for("agent-1", [e])
        self.assertEqual(exps, [])
        self.assertEqual(len(unver), 1)
        self.assertIn("no readable rating", unver[0].why)

    def test_a_refused_write_is_unverifiable_not_a_finding(self) -> None:
        e = entry("book.add", target=None, outcome="refused", params={"uuid": BOOK})
        exps, unver, _, _ = expectations.expectations_for("agent-2", [e])
        self.assertEqual(exps, [])
        self.assertIn("refused", unver[0].why)

    def test_an_unknown_action_is_unverifiable_and_names_itself(self) -> None:
        e = entry("shelf.rename", target=None, params={"name": "x"})
        exps, unver, tally, _ = expectations.expectations_for("agent-1", [e])
        self.assertEqual(exps, [])
        self.assertIn("shelf.rename", unver[0].why)
        self.assertEqual(tally[vocabulary.UNKNOWN], 1)

    def test_an_observation_produces_neither_expectation_nor_unverifiable(self) -> None:
        exps, unver, tally, _ = expectations.expectations_for("agent-1", [entry("book.open", target=BOOK)])
        self.assertEqual((exps, unver), ([], []))
        self.assertEqual(tally[vocabulary.OBSERVATION], 1)

    def test_progress_takes_its_axis_from_the_action_head(self) -> None:
        exps, _, _, _ = expectations.expectations_for(
            "agent-3", [entry("player.close", target=BOOK, params={"final_position_secs": 6869})]
        )
        self.assertEqual(exps[0].value["axis"], "audio")
        self.assertEqual(exps[0].value["seconds"], 6869.0)


class CompareTests(unittest.TestCase):
    def _expect(self, family: str, value: Any, target: str | None = BOOK) -> expectations.Expectation:
        return expectations.Expectation("agent-1", 7, family, family, target, "expected", value)

    def test_a_matching_rating_produces_no_finding(self) -> None:
        state = FakeState(ratings={BOOK: 4.5})
        self.assertIsNone(compare.check(self._expect("rating", 4.5), state))

    def test_a_deleted_rating_is_reported_missing(self) -> None:
        found = compare.check(self._expect("rating", 4.5), FakeState(ratings={}))
        self.assertIsNotNone(found)
        self.assertEqual(found.kind, compare.MISSING)
        self.assertEqual(found.replay_from, 7)

    def test_a_changed_rating_is_reported_as_a_mismatch(self) -> None:
        found = compare.check(self._expect("rating", 4.5), FakeState(ratings={BOOK: 2.0}))
        self.assertEqual(found.kind, compare.MISMATCH)
        self.assertIn("2 of 5", found.observed)

    def test_an_absent_read_status_row_reads_as_unread(self) -> None:
        # `shared::ReadStatus::Unread` documents the missing row as unread, so
        # an agent that journalled a return to unread lost nothing.
        self.assertIsNone(compare.check(self._expect("read_status", "unread"), FakeState(statuses={})))

    def test_a_missing_read_status_row_is_a_finding_when_reading_was_claimed(self) -> None:
        found = compare.check(self._expect("read_status", "reading"), FakeState(statuses={}))
        self.assertEqual(found.kind, compare.MISSING)

    def test_progress_is_checked_on_the_axis_the_entry_named(self) -> None:
        audio = {"audio_position_seconds": 4174.0, "epub_cfi": None}
        state = FakeState(progress={(BOOK, "audio"): audio})
        self.assertIsNone(compare.check(self._expect("progress", {"axis": "audio"}), state))
        # The same book with only an ebook row must not satisfy an audio claim.
        ebook_only = FakeState(progress={(BOOK, "ebook"): {"epub_cfi": "epubcfi(/6)"}})
        self.assertEqual(compare.check(self._expect("progress", {"axis": "audio"}), ebook_only).kind, compare.MISSING)

    def test_progress_does_not_compare_the_exact_position(self) -> None:
        # The player keeps writing after the journal line is appended, so an
        # exact comparison would fail on a healthy run.
        state = FakeState(progress={(BOOK, "audio"): {"audio_position_seconds": 9999.0}})
        self.assertIsNone(compare.check(self._expect("progress", {"axis": "audio", "seconds": 10.0}), state))

    def test_a_journal_entry_matches_through_reflowed_whitespace(self) -> None:
        state = FakeState(journals={BOOK: [{"body_md": "one\n\ntwo   three"}]})
        self.assertIsNone(compare.check(self._expect("journal", "one two three"), state))

    def test_two_copies_of_one_journal_entry_are_a_duplicate(self) -> None:
        state = FakeState(journals={BOOK: [{"body_md": "hello"}, {"body_md": "hello"}]})
        self.assertEqual(compare.check(self._expect("journal", "hello"), state).kind, compare.DUPLICATE)

    def test_an_edited_journal_entry_is_a_mismatch_not_a_miss(self) -> None:
        exp = expectations.Expectation(
            "agent-1", 7, "journal", "journal entry", BOOK, "expected", "new text", {"phrase": "brine-lantern"}
        )
        state = FakeState(journals={BOOK: [{"body_md": "old text with brine-lantern in it"}]})
        self.assertEqual(compare.check(exp, state).kind, compare.MISMATCH)

    def test_a_wishlist_entry_that_is_gone_is_missing(self) -> None:
        found = compare.check(self._expect("wishlist", "a-uuid", target=None), FakeState(wishlist=[]))
        self.assertEqual(found.kind, compare.MISSING)

    def test_unexpected_needs_a_baseline(self) -> None:
        state = FakeState(ratings={BOOK: 5.0}, library=[BOOK])
        self.assertEqual(compare.unexpected("agent-1", state, None, claims_of()), [])

    def test_unexpected_reports_a_rating_the_journal_never_claimed(self) -> None:
        baseline = {"library": [BOOK], "actors": {"agent-1": {"books": {BOOK: {"rating": None}}}}}
        state = FakeState(ratings={BOOK: 5.0}, statuses={}, journals={}, shelves=[])
        found = compare.unexpected("agent-1", state, baseline, claims_of())
        self.assertEqual([f.kind for f in found], [compare.UNEXPECTED])

    def test_unexpected_stays_quiet_about_a_shelf_the_journal_claimed(self) -> None:
        baseline = {"library": [], "actors": {"agent-2": {"books": {}, "shelves": []}}}
        state = FakeState(shelves=[{"name": "agent-2 shortlist", "id": 7}])
        claimed = claims_of(shelf_names={"agent-2 shortlist"})
        self.assertEqual(compare.unexpected("agent-2", state, baseline, claimed), [])
        # …but a second, unclaimed shelf on the same actor is still reported.
        state2 = FakeState(shelves=[{"name": "agent-2 shortlist"}, {"name": "mystery pile"}])
        found = compare.unexpected("agent-2", state2, baseline, claimed)
        self.assertEqual([f.observed for f in found], ["shelf 'mystery pile'"])

    def test_unexpected_stays_quiet_about_state_the_baseline_already_held(self) -> None:
        baseline = {"library": [BOOK], "actors": {"agent-1": {"books": {BOOK: {"rating": 5.0}}, "shelves": []}}}
        state = FakeState(ratings={BOOK: 5.0}, statuses={}, journals={}, shelves=[])
        self.assertEqual(compare.unexpected("agent-1", state, baseline, claims_of()), [])


class ClaimsTests(unittest.TestCase):
    """FP-4 / FP-5: `unexpected` must subtract every slot the journal addressed."""

    def _baseline(self, actor: str = "agent-1") -> dict:
        return {"library": [BOOK], "actors": {actor: {"books": {BOOK: {"rating": None}}, "shelves": []}}}

    def test_an_unjudged_write_still_claims_its_slot_against_unexpected(self) -> None:
        # FP-4: the rating params are unreadable, so no expectation is folded —
        # but the write usually landed, and re-reporting it as `unexpected`
        # would contradict the journal line that names it.
        entries = [entry("rating.set", target=BOOK, params={"how": "clicked the stars"})]
        exps, unver, _, claims = expectations.expectations_for("agent-1", entries)
        self.assertEqual(exps, [])
        self.assertEqual(len(unver), 1)
        self.assertIn(("rating", BOOK), claims.slots)
        state = FakeState(ratings={BOOK: 5.0}, statuses={}, journals={}, shelves=[])
        self.assertEqual(compare.unexpected("agent-1", state, self._baseline(), claims), [])

    def test_an_unknown_shelf_verb_suppresses_the_shelf_sweep(self) -> None:
        # FP-4: `shelf.rename` is UNKNOWN — the audit cannot say which shelf
        # it touched, so no shelf-level surprise is sound to report.
        entries = [entry("shelf.rename", params={})]
        _, _, _, claims = expectations.expectations_for("agent-2", entries)
        self.assertTrue(claims.shelf_any)
        baseline = {"library": [], "actors": {"agent-2": {"books": {}, "shelves": []}}}
        state = FakeState(shelves=[{"name": "renamed pile"}])
        self.assertEqual(compare.unexpected("agent-2", state, baseline, claims), [])

    def test_a_named_unknown_shelf_verb_claims_just_that_name(self) -> None:
        entries = [entry("shelf.rename", params={"name": "new name"})]
        _, _, _, claims = expectations.expectations_for("agent-2", entries)
        self.assertFalse(claims.shelf_any)
        self.assertIn("new name", claims.shelf_names)

    def test_opening_a_book_claims_its_read_status_slot(self) -> None:
        # FP-5: the reading surfaces auto-write read status client-side
        # (read_status_auto.rs) — merely opening a book moves unread→reading
        # with nothing journalled, and that must not read as `unexpected`.
        for action in ("book.open", "reader.open", "player.play"):
            entries = [entry(action, target=BOOK)]
            _, _, _, claims = expectations.expectations_for("agent-1", entries)
            self.assertIn(("read_status", BOOK), claims.slots, action)
        state = FakeState(ratings={}, statuses={BOOK: "reading"}, journals={}, shelves=[])
        baseline = {"library": [BOOK], "actors": {"agent-1": {"books": {BOOK: {"read_status": None}}, "shelves": []}}}
        _, _, _, claims = expectations.expectations_for("agent-1", [entry("book.open", target=BOOK)])
        self.assertEqual(compare.unexpected("agent-1", state, baseline, claims), [])

    def test_a_status_write_the_journal_never_made_is_still_reported(self) -> None:
        # The FP-5 claim is scoped to the book that was opened — a status
        # change on a book nothing touched still surfaces.
        other = "00000000-0000-0000-0000-000000000001"
        _, _, _, claims = expectations.expectations_for("agent-1", [entry("book.open", target=BOOK)])
        state = FakeState(ratings={}, statuses={other: "finished"}, journals={}, shelves=[])
        baseline = {
            "library": [BOOK, other],
            "actors": {"agent-1": {"books": {BOOK: {}, other: {"read_status": None}}, "shelves": []}},
        }
        found = compare.unexpected("agent-1", state, baseline, claims)
        self.assertEqual([(f.kind, f.target) for f in found], [(compare.UNEXPECTED, other)])


class FoldEditTests(unittest.TestCase):
    """FP-1/2/3, SM-1/2/3: edits and deletes must supersede what they name."""

    def test_journal_update_pops_the_entry_it_names_not_the_most_recent(self) -> None:
        # FP-1: create A, create B, edit A. The edit names A by its prior
        # text; popping "most recent" would cancel B and leave A stale.
        entries = [
            entry("journal.create", seq=1, target=BOOK, params={"entry_text_verbatim": "first entry"}),
            entry("journal.create", seq=2, target=BOOK, params={"entry_text_verbatim": "second entry"}),
            entry(
                "journal.update", seq=3, target=BOOK,
                params={"before_verbatim": "first entry", "after_verbatim": "first entry, edited"},
            ),
        ]
        exps, unver, _, _ = expectations.expectations_for("agent-1", entries)
        self.assertEqual(sorted(e.value for e in exps), ["first entry, edited", "second entry"])
        self.assertEqual(unver, [])

    def test_a_note_edit_supersedes_the_highlight_it_edited(self) -> None:
        # FP-2: the prior is named by its OLD note; the new note is what the
        # replacement asserts, and the untouched quote survives the merge.
        entries = [
            entry("highlight.create", seq=1, target=BOOK, params={"note_text": "old note", "quote": "the passage"}),
            entry("highlight.note", seq=2, target=BOOK, params={"old_note": "old note", "note_text": "new note"}),
        ]
        exps, unver, _, _ = expectations.expectations_for("agent-1", entries)
        self.assertEqual(len(exps), 1)
        self.assertEqual(exps[0].value["note"], "new note")
        self.assertEqual(exps[0].value["quote"], "the passage")
        self.assertEqual(unver, [])

    def test_a_note_edit_finds_the_prior_by_its_quote(self) -> None:
        entries = [
            entry("highlight.create", seq=1, target=BOOK, params={"note_text": "old", "quote": "the passage"}),
            entry("highlight.note", seq=2, target=BOOK, params={"note_text": "new", "quote": "the passage"}),
        ]
        exps, _, _, _ = expectations.expectations_for("agent-1", entries)
        self.assertEqual([e.value["note"] for e in exps], ["new"])

    def test_a_highlight_delete_matches_on_the_quote(self) -> None:
        # FP-3: the delete names the quote, not the note — it must still find
        # the create rather than leaving a stale `missing` plus a skip.
        entries = [
            entry("highlight.create", seq=1, target=BOOK, params={"note_text": "keep an eye", "quote": "the passage"}),
            entry("highlight.delete", seq=2, target=BOOK, params={"quote": "the passage"}),
        ]
        exps, unver, _, _ = expectations.expectations_for("agent-1", entries)
        self.assertEqual(exps, [])
        self.assertEqual(unver, [])

    def test_dual_format_progress_folds_per_axis(self) -> None:
        # SM-1: a dual-format book keeps one position row per format; folding
        # reader and player statements together leaves one axis unchecked.
        entries = [
            entry("reader.progress", seq=1, target=BOOK, params={"app_shows": "p. 3 of 22 · 11%"}),
            entry("player.seek", seq=2, target=BOOK, params={"to_secs": 120.0}),
        ]
        exps, _, _, _ = expectations.expectations_for("agent-1", entries)
        self.assertEqual(sorted(e.value["axis"] for e in exps), ["audio", "ebook"])

    def test_a_colour_only_edit_keeps_the_note_it_cannot_name(self) -> None:
        # SM-2: a colour change names nothing identifying; with one candidate
        # the reference is unambiguous, and the merge must keep the note.
        entries = [
            entry("highlight.create", seq=1, target=BOOK, params={"note_text": "keep", "colour": "green"}),
            entry("highlight.colour", seq=2, target=BOOK, params={"colour": "blue"}),
        ]
        exps, _, _, _ = expectations.expectations_for("agent-1", entries)
        self.assertEqual(len(exps), 1)
        self.assertEqual(exps[0].value["note"], "keep")
        self.assertEqual(exps[0].value["colour"], "blue")

    def test_an_ambiguous_edit_is_declined_not_guessed(self) -> None:
        entries = [
            entry("highlight.create", seq=1, target=BOOK, params={"colour": "green"}),
            entry("highlight.create", seq=2, target=BOOK, params={"colour": "green"}),
            entry("highlight.colour", seq=3, target=BOOK, params={"colour": "blue"}),
        ]
        exps, unver, _, _ = expectations.expectations_for("agent-1", entries)
        self.assertEqual(len(exps), 2)
        self.assertEqual(len(unver), 1)
        self.assertIn("cannot attribute", unver[0].why)

    def test_highlight_colour_is_compared_against_the_row(self) -> None:
        # SM-2: colour was never compared on any path.
        exp = expectations.Expectation(
            "agent-1", 2, "highlight", "highlight", BOOK, "x", {"note": "keep", "colour": "blue"}
        )
        wrong = FakeState(highlights={BOOK: [{"note": "keep", "text": None, "color": "green"}]})
        found = compare.check(exp, wrong)
        self.assertIsNotNone(found)
        self.assertEqual(found.kind, compare.MISMATCH)
        right = FakeState(highlights={BOOK: [{"note": "keep", "text": None, "color": "blue"}]})
        self.assertIsNone(compare.check(exp, right))

    def test_bookmark_keys_identify_the_row(self) -> None:
        # SM-3: bookmarks used to fold with no keys at all, so every row
        # matched and a lost bookmark could never be reported.
        entries = [
            entry("bookmark.create", seq=1, target=BOOK, params={"title": "chapter one"}),
            entry("bookmark.create", seq=2, target=BOOK, params={"title": "chapter two"}),
        ]
        exps, _, _, _ = expectations.expectations_for("agent-1", entries)
        state = FakeState(bookmarks={BOOK: [{"position": "epubcfi(/6/2)", "title": "chapter one"}]})
        outcomes = [compare.check(e, state) for e in exps]
        kinds = [f.kind if f else None for f in outcomes]
        self.assertEqual(kinds, [None, compare.MISMATCH])


class ContainmentTests(unittest.TestCase):
    """PY-4: one bad uuid or one dead login must not destroy the report."""

    def test_one_failing_state_read_costs_one_expectation_not_the_report(self) -> None:
        bad = "not-a-uuid"

        class OneBadUuid(FakeState):
            def rating(self, uuid: str) -> Any:
                if uuid == bad:
                    raise ApiError(f"POST /api/rpc/ratings/get -> HTTP 500 for {uuid}")
                return super().rating(uuid)

        good = expectations.Expectation("agent-1", 1, "rating", "rating", BOOK, "x", 4.5)
        broken = expectations.Expectation("agent-1", 2, "rating", "rating", bad, "x", 3.0)
        state = OneBadUuid(ratings={BOOK: 4.5})
        findings, failed = compare.check_all([good, broken], state)
        self.assertEqual(findings, [])
        self.assertEqual([exp.seq for exp, _ in failed], [2])
        self.assertIn("state read failed", failed[0][1])

    def test_a_dead_login_is_contained_to_its_actor(self) -> None:
        import audit
        from audit_lib.client import Account

        # Port 1 refuses instantly; the helper must answer, not raise.
        reader, error = audit.actor_reader("http://127.0.0.1:1", Account("agent-9", "u", "p"))
        self.assertIsNone(reader)
        self.assertIn("agent-9", error or "")


class ReplayTests(unittest.TestCase):
    def test_wishlist_is_refused_with_its_reason(self) -> None:
        exp = expectations.Expectation("agent-3", 4, "wishlist", "wishlist entry", None, "x", "a-uuid")
        step = replay.replay_one(exp, FakeState())
        self.assertEqual(step.action, "refused")
        self.assertIn("ISBN-lookup", step.detail)

    def test_a_percent_only_epub_position_replays_as_a_valid_update(self) -> None:
        # ProgressUpdate::validate accepts a cfi OR a percent for epub — a Kobo
        # has no CFI to give — so percent-only is a first-class position and
        # must replay rather than be refused.
        payload = replay._progress_payload(BOOK, {"axis": "ebook", "percent": 11.0, "cfi": None})
        self.assertEqual(payload, {"book_uuid": BOOK, "format": "epub", "progress_percent": 11})

    def test_an_epub_position_with_neither_cfi_nor_percent_is_refused(self) -> None:
        exp = expectations.Expectation(
            "agent-1", 15, "progress", "progress", BOOK, "x", {"axis": "ebook", "percent": None, "cfi": None}
        )
        step = replay.replay_one(exp, FakeState())
        self.assertEqual(step.action, "refused")

    def test_an_audio_position_builds_a_valid_progress_payload(self) -> None:
        payload = replay._progress_payload(BOOK, {"axis": "audio", "seconds": 42.5})
        self.assertEqual(payload, {"book_uuid": BOOK, "format": "audio", "audio_position_seconds": 42.5})

    def test_replay_sets_playback_rate_with_the_rpc_shape(self) -> None:
        # PY-1: the server function takes (uuid, update) and the update's
        # field is `playback_rate` — the old flat body could never succeed.
        client = FakeClient()
        exp = expectations.Expectation("agent-2", 51, "playback_rate", "playback rate", BOOK, "x", 1.5)
        step = replay.replay_one(exp, FakeState(client=client))
        self.assertTrue(step.ok)
        self.assertEqual(
            client.calls,
            [("POST", "/api/rpc/audiobooks/playback-rate/set", {"uuid": BOOK, "update": {"playback_rate": 1.5}})],
        )

    def test_replay_lands_the_note_through_its_own_patch(self) -> None:
        # PY-2: CreateHighlight has no note field and unknown fields are
        # silently dropped — an inline note would 200 and vanish.
        client = FakeClient(responses={"/api/highlights": (200, '{"id": 7}')})
        exp = expectations.Expectation(
            "agent-1", 9, "highlight", "highlight", BOOK, "x",
            {"note": "check this", "quote": "the passage", "colour": "green", "cfi": "epubcfi(/6/4!/4/2)"},
        )
        step = replay.replay_one(exp, FakeState(client=client))
        self.assertTrue(step.ok)
        methods_paths = [(m, path) for m, path, _ in client.calls]
        self.assertEqual(
            methods_paths,
            [("POST", "/api/highlights"), ("PATCH", "/api/highlights/7/note")],
        )
        create_payload = client.calls[0][2]
        self.assertNotIn("note", create_payload)
        self.assertEqual(client.calls[1][2], {"note": "check this"})

    def test_replay_refusal_names_the_missing_machine_location(self) -> None:
        # PY-3: without a cfi the refusal must say what capability is absent,
        # not imply the journal was deficient in some unstated way.
        exp = expectations.Expectation(
            "agent-1", 9, "highlight", "highlight", BOOK, "x", {"note": "n", "quote": "q", "colour": None, "cfi": None}
        )
        step = replay.replay_one(exp, FakeState(client=FakeClient()))
        self.assertEqual(step.action, "refused")
        self.assertIn("epubcfi", step.detail)

    def test_bookmark_replay_uses_the_journalled_position_and_title(self) -> None:
        client = FakeClient()
        exp = expectations.Expectation(
            "agent-1", 4, "bookmark", "bookmark", BOOK, "x", {"label": "chapter one", "position": "epubcfi(/6/2)"}
        )
        step = replay.replay_one(exp, FakeState(client=client))
        self.assertTrue(step.ok)
        self.assertEqual(
            client.calls,
            [("POST", "/api/bookmarks", {"book_uuid": BOOK, "position": "epubcfi(/6/2)", "title": "chapter one"})],
        )


def _append_many(path: str, actor: str, count: int) -> None:
    """Worker for the concurrency test — one process per actor."""
    for i in range(count):
        journal.append(path, {"actor": actor, "action": "note", "params": {"i": i, "pad": "x" * 4000}})


class JournalTests(unittest.TestCase):
    def test_iter_entries_fails_loudly_on_a_torn_line(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            p = Path(tmp) / "journal.jsonl"
            p.write_text('{"actor":"a","seq":1}\n{"actor":"b",\n', encoding="utf-8")
            with self.assertRaises(journal.JournalError):
                journal.read_entries(p)

    def test_actor_entries_orders_by_seq_not_by_file_order(self) -> None:
        entries = [entry("note", actor="a", seq=3), entry("note", actor="b", seq=1), entry("note", actor="a", seq=1)]
        self.assertEqual([e.seq for e in journal.actor_entries(entries, "a")], [1, 3])

    def test_append_mints_a_monotonic_seq_per_actor(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            p = Path(tmp) / "journal.jsonl"
            self.assertEqual(journal.append(p, {"actor": "agent-1", "action": "note"})["seq"], 1)
            self.assertEqual(journal.append(p, {"actor": "agent-2", "action": "note"})["seq"], 1)
            self.assertEqual(journal.append(p, {"actor": "agent-1", "action": "note"})["seq"], 2)

    def test_concurrent_appends_never_interleave_or_truncate(self) -> None:
        """AC1: three agents, one file, oversized records, no torn lines."""
        with tempfile.TemporaryDirectory() as tmp:
            p = Path(tmp) / "journal.jsonl"
            p.touch()
            ctx = multiprocessing.get_context("spawn")
            procs = [ctx.Process(target=_append_many, args=(str(p), f"agent-{n}", 40)) for n in (1, 2, 3)]
            for proc in procs:
                proc.start()
            for proc in procs:
                proc.join(120)
                self.assertEqual(proc.exitcode, 0)

            lines = [ln for ln in p.read_text(encoding="utf-8").splitlines() if ln.strip()]
            self.assertEqual(len(lines), 120)
            for line in lines:
                json.loads(line)  # raises if a record was split or spliced
            entries = journal.read_entries(p)
            for actor in ("agent-1", "agent-2", "agent-3"):
                seqs = sorted(e.seq for e in entries if e.actor == actor)
                self.assertEqual(seqs, list(range(1, 41)), f"{actor} lost or repeated a seq")

    def test_journal_path_rejects_an_implausible_run_id(self) -> None:
        with self.assertRaises(journal.JournalError):
            journal.journal_path("../../etc", "/tmp")


class EnvTests(unittest.TestCase):
    def test_parse_reads_only_the_exploration_keys(self) -> None:
        got = env.parse("HARDCOVER_API_KEY=secret\nOMNIBUS_EXPLORE_URL=https://x\n# comment\n")
        self.assertEqual(got, {"OMNIBUS_EXPLORE_URL": "https://x"})

    def test_parse_expands_the_home_form_env_example_recommends(self) -> None:
        got = env.parse("OMNIBUS_EXPLORE_JOURNAL_DIR=$HOME/.omnibus-explore/journals\n")
        self.assertTrue(got["OMNIBUS_EXPLORE_JOURNAL_DIR"].endswith("/.omnibus-explore/journals"))
        self.assertNotIn("$HOME", got["OMNIBUS_EXPLORE_JOURNAL_DIR"])

    def test_parse_strips_surrounding_quotes(self) -> None:
        self.assertEqual(env.parse('OMNIBUS_EXPLORE_URL="https://x"\n')["OMNIBUS_EXPLORE_URL"], "https://x")

    def test_load_never_clobbers_an_exported_value(self) -> None:
        existing = {"OMNIBUS_EXPLORE_URL": "https://already-set"}
        env.load(existing)
        self.assertEqual(existing["OMNIBUS_EXPLORE_URL"], "https://already-set")


class AccountTests(unittest.TestCase):
    def test_load_accounts_reads_provision_output(self) -> None:
        raw = [{"actor": "agent-1", "username": "explorer-1", "password": "p", "action": "reused"}]
        self.assertEqual(load_accounts(raw)["agent-1"].username, "explorer-1")

    def test_load_accounts_rejects_a_row_missing_a_field(self) -> None:
        with self.assertRaises(ApiError):
            load_accounts([{"actor": "agent-1"}])


if __name__ == "__main__":
    unittest.main()
