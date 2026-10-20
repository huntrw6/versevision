import pytest

from versevision.follow import Follower
from versevision.listen import Detector
from versevision.scripture import ScriptureIndex, Verse

# Public-domain KJV wording for a handful of verses (the real index holds the whole KJV and the BSB).
KJV = {
    ("PSA", "Psalms", 23): [
        "The LORD is my shepherd; I shall not want.",
        "He maketh me to lie down in green pastures: he leadeth me beside the still waters.",
        "He restoreth my soul: he leadeth me in the paths of righteousness for his name's sake.",
        "Yea, though I walk through the valley of the shadow of death, I will fear no evil: for thou art with me; thy rod and thy staff they comfort me.",
    ],
    ("JHN", "John", 3): [""] * 15 + [
        "For God so loved the world, that he gave his only begotten Son, that whosoever believeth in him should not perish, but have everlasting life.",
        "For God sent not his Son into the world to condemn the world; but that the world through him might be saved.",
        "He that believeth on him is not condemned: but he that believeth not is condemned already, because he hath not believed in the name of the only begotten Son of God.",
    ],
    ("ROM", "Romans", 8): [""] * 27 + [
        "And we know that all things work together for good to them that love God, to them who are the called according to his purpose.",
    ],
    ("PRO", "Proverbs", 3): [""] * 4 + [
        "Trust in the LORD with all thine heart; and lean not unto thine own understanding.",
        "In all thy ways acknowledge him, and he shall direct thy paths.",
    ],
}


# Berean Standard Bible wording (public domain), which is closer to what ESV/NIV congregations hear
BSB = {
    ("PSA", "Psalms", 23): [
        "The LORD is my shepherd; I shall not want.",
        "He makes me lie down in green pastures; He leads me beside quiet waters.",
        "He restores my soul; He guides me in the paths of righteousness for the sake of His name.",
    ],
    ("JHN", "John", 3): [""] * 15 + [
        "For God so loved the world that He gave His one and only Son, that everyone who believes in Him shall not perish but have eternal life.",
        "For God did not send His Son into the world to condemn the world, but to save the world through Him.",
    ],
    ("PRO", "Proverbs", 3): [""] * 4 + [
        "Trust in the LORD with all your heart, and do not lean on your own understanding.",
        "Acknowledge Him in all your ways, and He will make your paths straight.",
    ],
}


def make_index():
    bibles = {}
    for name, source in (("KJV", KJV), ("BSB", BSB)):
        verses = []
        for (bid, book, chapter), texts in source.items():
            for n, text in enumerate(texts, 1):
                if text:
                    verses.append(Verse(bid, book, chapter, n, text))
        bibles[name] = verses
    return ScriptureIndex(bibles)


def say(follower, *lines):
    """lines: (t, kind, text). Returns every action produced."""
    out = []
    for t, kind, text in lines:
        out += follower.feed({"kind": kind, "t": t, "text": text})
    return out


def shown(actions):
    return [(a.reference, a.reason.split(" (")[0]) for a in actions if a.kind == "show"]


@pytest.fixture
def idx():
    return make_index()


def test_a_verse_range_follows_the_speaker_one_verse_at_a_time(idx):
    f = Follower(index=idx, hold=30)
    acts = say(f,
               (1.0, "partial", "turn to Psalm 23 1 through 3"), (2.2, "partial", "turn to Psalm 23 1 through 3 and"), (3.0, "partial", "turn to Psalm 23 1 through 3 and read along"),
               (4.5, "partial", "turn to Psalm 23 1 through 3 and read along the Lord is my shepherd"),
               (6.5, "partial", "the Lord is my shepherd I shall not want he makes me lie down"),
               (8.0, "partial", "the Lord is my shepherd I shall not want he makes me lie down in green pastures he leads me beside the still waters he restores my soul"),
               (10.0, "final", "the Lord is my shepherd I shall not want he makes me lie down in green pastures he leads me beside the still waters he restores my soul he leads me in paths"))
    got = shown(acts)
    assert got[0][0] == "Psalms 23:1" and "reference" in got[0][1]
    assert [g[0] for g in got] == ["Psalms 23:1", "Psalms 23:2", "Psalms 23:3"]
    assert all(("began reading" in g[1] or "assumed" in g[1]) for g in got[1:])


def test_it_does_not_run_past_the_end_of_a_named_range(idx):
    f = Follower(index=idx, hold=30)
    acts = say(f, (1.0, "final", "look at Psalm 23 1 through 2"), (3.0, "partial", "the Lord is my shepherd I shall not want he makes me lie down in green pastures"),
               (6.0, "partial", "he leads me in the paths of righteousness for his name's sake"),
               (9.0, "partial", "yea though I walk through the valley of the shadow of death"))
    assert [a.reference for a in acts if a.kind == "show"] == ["Psalms 23:1", "Psalms 23:2"]


def test_a_single_verse_reference_keeps_following_when_the_speaker_reads_on(idx):
    f = Follower(index=idx, hold=30)
    acts = say(f, (1.0, "final", "turn with me to John 3 16"),
               (3.0, "partial", "for God so loved the world that he gave his only begotten son that whosoever believes in him should not perish"),
               (8.0, "partial", "that whosoever believes in him should not perish but have everlasting life for God sent not his son into the world to condemn"))
    assert [a.reference for a in acts if a.kind == "show"] == ["John 3:16", "John 3:17"]


def test_a_verse_read_without_its_reference_is_recognised_from_its_words(idx):
    f = Follower(index=idx, hold=30)
    acts = say(f, (1.0, "partial", "trust in the Lord with all your heart"), (2.2, "partial", "trust in the Lord with all your heart and do not lean on your own understanding"),
               (3.4, "partial", "trust in the Lord with all your heart and do not lean on your own understanding"))
    got = shown(acts)
    assert got and got[0][0] == "Proverbs 3:5" and "verse itself" in got[0][1]


def test_then_it_follows_the_content_found_verse_to_the_next_one(idx):
    f = Follower(index=idx, hold=30)
    acts = say(f, (1.0, "partial", "trust in the Lord with all thine heart and lean not unto thine own understanding"),
               (2.2, "partial", "trust in the Lord with all thine heart and lean not unto thine own understanding"),
               (6.0, "partial", "in all thy ways acknowledge him and he shall direct thy paths"))
    assert [a.reference for a in acts if a.kind == "show"] == ["Proverbs 3:5", "Proverbs 3:6"]


def test_ordinary_talk_shows_nothing(idx):
    f = Follower(index=idx)
    acts = say(f, (1.0, "partial", "we have ten minutes left and the second point in our outline is about prayer in the life of the church"),
               (3.0, "final", "we have ten minutes left and the second point in our outline is about prayer in the life of the church"))
    assert acts == []


def test_a_loose_paraphrase_is_not_mistaken_for_a_verse(idx):
    f = Follower(index=idx)
    acts = say(f, (1.0, "partial", "God loved the world so much that he sent his son so that people who trust in him would live forever"),
               (3.0, "final", "God loved the world so much that he sent his son so that people who trust in him would live forever"))
    assert acts == []


def test_the_screen_clears_after_the_hold_and_reading_keeps_it_up(idx):
    f = Follower(index=idx, hold=10)
    say(f, (1.0, "final", "John 3 16"))
    assert [a.kind for a in f.tick(9.0)] == []
    say(f, (9.5, "partial", "for God so loved the world that he gave his only begotten son"))   # still reading: the hold restarts
    assert f.tick(15.0) == []
    clear = f.tick(20.0)
    assert [a.kind for a in clear] == ["clear"] and f.current is None


def test_the_same_verse_is_not_shown_twice_inside_the_cooldown(idx):
    f = Follower(index=idx, hold=5, cooldown=30)
    first = say(f, (1.0, "final", "John 3 16"))
    f.tick(20.0)                                       # cleared
    again = say(f, (21.0, "final", "back to John 3 16"))
    assert [a.reference for a in first if a.kind == "show"] == ["John 3:16"] and [a for a in again if a.kind == "show"] == []


def test_a_new_reference_replaces_what_is_on_screen(idx):
    f = Follower(index=idx)
    acts = say(f, (1.0, "final", "John 3 16"), (6.0, "final", "now Romans 8 28"))
    assert [a.reference for a in acts if a.kind == "show"] == ["John 3:16", "Romans 8:28"]


def test_without_a_scripture_index_references_still_work_and_nothing_else_happens():
    f = Follower(index=None)
    acts = say(f, (1.0, "final", "John 3 16"), (3.0, "partial", "for God so loved the world that he gave his only begotten son"))
    assert [a.reference for a in acts if a.kind == "show"] == ["John 3:16"]


def test_follow_and_content_can_be_switched_off(idx):
    f = Follower(index=idx, follow=False, content=False)
    acts = say(f, (1.0, "final", "John 3 16"), (3.0, "partial", "for God sent not his son into the world to condemn the world"),
               (5.0, "partial", "trust in the Lord with all thine heart and lean not unto thine own understanding"))
    assert [a.reference for a in acts if a.kind == "show"] == ["John 3:16"]


# ---------------------------------------------------------------- ties: parallel passages
PARALLEL = {
    ("MAT", "Matthew", 4): [""] * 3 + ["But he answered and said, It is written, Man shall not live by bread alone, but by every word that proceedeth out of the mouth of God."],
    ("LUK", "Luke", 4): [""] * 3 + ["And Jesus answered him, saying, It is written, That man shall not live by bread alone, but by every word that proceedeth out of the mouth of God."],
}


def parallel_index():
    verses = [Verse(b, n, c, i, x) for (b, n, c), texts in PARALLEL.items() for i, x in enumerate(texts, 1) if x]
    return ScriptureIndex({"KJV": verses})


def test_when_two_verses_fit_equally_the_earlier_one_in_the_bible_is_chosen():
    m = parallel_index().match("it is written man shall not live by bread alone but by every word that proceedeth out of the mouth of God")
    assert m is not None and m.tied and m.verse.reference == "Matthew 4:4" and [a.reference for a in m.alternatives] == ["Luke 4:4"]


def test_context_overrides_the_earlier_default():
    m = parallel_index().match("it is written man shall not live by bread alone but by every word that proceedeth out of the mouth of God", prefer=("LUK", 4))
    assert m is not None and not m.tied and m.verse.reference == "Luke 4:4"


def test_more_words_that_only_fit_one_verse_beat_the_default():
    idx = parallel_index()
    f = Follower(index=idx, hold=30)
    acts = say(f, (1.0, "partial", "it is written man shall not live by bread alone but by every word that proceedeth out of the mouth of God"),
               (2.3, "partial", "it is written man shall not live by bread alone but by every word that proceedeth out of the mouth of God"),
               (6.0, "partial", "and Jesus answered him saying it is written that man shall not live by bread alone but by every word that proceedeth out of the mouth of God"),
               (7.4, "partial", "and Jesus answered him saying it is written that man shall not live by bread alone but by every word that proceedeth out of the mouth of God"))
    shows = [a for a in acts if a.kind == "show"]
    assert [a.reference for a in shows] == ["Matthew 4:4", "Luke 4:4"] and shows[0].tentative and not shows[1].tentative


def test_a_stock_phrase_found_in_many_verses_is_not_treated_as_a_quotation():
    verses = [Verse("EXO", "Exodus", 6 + i, 1, "And the LORD spoke unto Moses, saying, Speak unto the children of Israel and say unto them these words") for i in range(6)]
    assert ScriptureIndex({"KJV": verses}).match("and the Lord spoke unto Moses saying speak unto the children of Israel and say unto them these words") is None


def test_the_first_show_of_a_named_range_carries_the_range_so_it_can_be_loaded_at_once(idx):
    f = Follower(index=idx, hold=30)
    acts = say(f, (1.0, "final", "look at Psalm 23 1 through 3"))
    first = [a for a in acts if a.kind == "show"][0]
    assert first.reference == "Psalms 23:1" and first.range_end == (23, 3)
    more = say(f, (4.0, "partial", "the Lord is my shepherd I shall not want he makes me lie down in green pastures"))
    assert [a.range_end for a in more if a.kind == "show"] == [None]


def test_propresenter_usx_bibles_are_read_without_footnotes_or_headings(tmp_path):
    import zipfile
    from versevision.references.books import BOOKS
    from versevision.scripture import load_propresenter_bibles
    usx = ('<?xml version="1.0" encoding="utf-8"?><usx version="2.6"><book code="JHN" style="id" /><para style="h">John</para>'
           '<chapter number="3" style="c" /><para style="s1">God So Loved the World</para><para style="p">'
           '<verse number="16" style="v" /><note caller="+" style="x"><char style="xo">3:16 </char><char style="xt">Rom. 5:8</char></note>'
           'For God so loved the world, that he gave his only Son, <char style="wj">that whoever believes</char> in him <note style="f">footnote</note>should not perish.'
           '<verse number="17" style="v" />For God did not send his Son into the world.</para></usx>')
    path = tmp_path / "Test_English_Standard_Version_1.rvbible"
    with zipfile.ZipFile(path, "w") as z:
        for b in BOOKS:
            z.writestr(f"release/USX_1/{b.id}.usx", usx if b.id == "JHN" else "<usx><book code='X'/></usx>")
    got = load_propresenter_bibles(tmp_path)
    # a Bible with almost no verses is skipped by the size check, so look at the parser directly
    from versevision.scripture import _usx_book
    verses = _usx_book(usx.encode())
    assert verses[(3, 16)] == "For God so loved the world, that he gave his only Son, that whoever believes in him should not perish."
    assert verses[(3, 17)] == "For God did not send his Son into the world." and "God So Loved" not in " ".join(verses.values())
    assert got == {}


# ---------------------------------------------------------------- Bible version
def test_a_version_said_after_the_reference_is_carried_on_the_action(idx):
    f = Follower(index=idx)
    acts = say(f, (1.0, "final", "turn to John 3 16 in the NIV"))
    shown_ = [a for a in acts if a.kind == "show"][0]
    assert (shown_.reference, shown_.version, shown_.version_source) == ("John 3:16", "NIV", "said")


def test_no_version_said_means_none_is_requested(idx):
    f = Follower(index=idx)
    acts = say(f, (1.0, "final", "turn to John 3 16"))
    assert [a.version for a in acts if a.kind == "show"] == [""]


def test_a_version_is_recognised_from_the_wording_only_when_one_bible_clearly_fits_best():
    nivish = "For God so loved the world that he gave his one and only Son, that whoever believes in him shall not perish but have eternal life."
    esvish = "For God so loved the world, that he gave his only Son, that whoever believes in him should not perish but have eternal life."
    kjv = "For God so loved the world, that he gave his only begotten Son, that whosoever believeth in him should not perish, but have everlasting life."
    verses = lambda text: [Verse("JHN", "John", 3, 16, text)]
    idx = ScriptureIndex({"NIV": verses(nivish), "ESV": verses(esvish), "KJV": verses(kjv)})
    m = idx.match("for God so loved the world that he gave his one and only Son that whoever believes in him shall not perish but have eternal life")
    assert m and m.version == "NIV" and m.version_clear
    same = ScriptureIndex({"NIV": verses(esvish), "ESV": verses(esvish), "KJV": verses(esvish)})
    m2 = same.match("for God so loved the world that he gave his only Son that whoever believes in him should not perish but have eternal life")
    assert m2 and not m2.version_clear


def test_pp_version_names():
    from versevision.listen_cli import pp_version
    assert pp_version("NIV") == "New International Version" and pp_version("NASB") == "New American Standard Bible 1995"
    assert pp_version("MSG") == "" and pp_version("") == ""


def test_it_does_not_jump_ahead_when_a_phrase_from_the_next_verse_occurs_inside_the_current_one():
    v16 = "For God so loved the world, that he gave his one and only Son, that whoever believes in him shall not perish but have eternal life."
    v17 = "For God did not send his Son into the world to condemn the world, but to save the world through him."
    v18 = "Whoever believes in him is not condemned, but whoever does not believe stands condemned already."
    idx = ScriptureIndex({"NIV": [Verse("JHN", "John", 3, 16, v16), Verse("JHN", "John", 3, 17, v17), Verse("JHN", "John", 3, 18, v18)]})
    f = Follower(index=idx, hold=30)
    acts = say(f, (1.0, "final", "John 3 16"), (3.0, "partial", "his one and only son that whoever believes in him shall not perish but have eternal life"))
    assert [a.reference for a in acts if a.kind == "show"] == ["John 3:16"]
    more = say(f, (8.0, "partial", "shall not perish but have eternal life for God did not send his son into the world to condemn the world"))
    assert [a.reference for a in more if a.kind == "show"] == ["John 3:17"]


def test_a_version_named_just_after_the_verse_re_shows_it_in_that_version(idx):
    f = Follower(index=idx, hold=30)
    first = say(f, (1.0, "final", "Romans 8 28"))
    later = say(f, (3.5, "final", "in the New Living Translation"))
    shows = [a for a in first + later if a.kind == "show"]
    assert [(a.reference, a.version) for a in shows] == [("Romans 8:28", ""), ("Romans 8:28", "NLT")] and shows[1].late_version


def test_a_late_version_is_ignored_after_a_while_or_when_one_was_already_said(idx):
    f = Follower(index=idx, hold=60)
    say(f, (1.0, "final", "Romans 8 28 in the NIV"))
    assert [a for a in say(f, (3.0, "final", "the English Standard Version")) if a.kind == "show"] == []
    g = Follower(index=idx, hold=60)
    say(g, (1.0, "final", "Romans 8 28"))
    assert [a for a in say(g, (20.0, "final", "the New Living Translation")) if a.kind == "show"] == []


# ---------------------------------------------------------------- reading on through the chapter
def chapter_index():
    v = {
        3: "His divine power has granted to us all things that pertain to life and godliness, through the knowledge of him who called us to his own glory and excellence",
        4: "by which he has granted to us his precious and very great promises, so that through them you may become partakers of the divine nature",
        5: "For this reason make every effort to supplement your faith with virtue, and virtue with knowledge",
        6: "and knowledge with self-control, and self-control with steadfastness, and steadfastness with godliness",
        7: "and godliness with brotherly affection, and brotherly affection with love",
        8: "For if these qualities are yours and increasing, they keep you from being ineffective or unfruitful in the knowledge of our Lord Jesus Christ",
    }
    return ScriptureIndex({"ESV": [Verse("2PE", "2 Peter", 1, n, x) for n, x in v.items()]})


def test_the_next_verse_is_picked_up_from_just_its_first_words():
    f = Follower(index=chapter_index(), hold=30)
    say(f, (1.0, "final", "second Peter 1 verse 3"))
    acts = say(f, (6.0, "partial", "of him who called us to his own glory and excellence by which he has granted"))
    assert [a.reference for a in acts if a.kind == "show"] == ["2 Peter 1:4"]


def test_a_misheard_word_or_two_does_not_lose_the_place():
    f = Follower(index=chapter_index(), hold=30)
    say(f, (1.0, "final", "second Peter 1 verse 5"))
    acts = say(f, (6.0, "partial", "with knowledge and knowledge with self control and self control with steadfastness and steadfastness with goveliness"))
    assert [a.reference for a in acts if a.kind == "show"] == ["2 Peter 1:6"]


def test_it_keeps_the_place_after_the_screen_clears():
    f = Follower(index=chapter_index(), hold=5)
    say(f, (1.0, "final", "second Peter 1 verse 5"))
    assert [a.kind for a in f.tick(10.0)] == ["clear"]
    acts = say(f, (40.0, "partial", "and knowledge with self-control and self-control with steadfastness"))
    assert [a.reference for a in acts if a.kind == "show"] == ["2 Peter 1:6"]


def test_it_can_skip_a_verse_and_still_find_the_place():
    f = Follower(index=chapter_index(), hold=30)
    say(f, (1.0, "final", "second Peter 1 verse 4"))
    acts = say(f, (6.0, "partial", "and knowledge with self-control and self-control with steadfastness and steadfastness with godliness"))
    assert [a.reference for a in acts if a.kind == "show"] == ["2 Peter 1:6"]


def test_a_few_words_matching_the_end_of_a_later_verse_do_not_move_it():
    f = Follower(index=chapter_index(), hold=30)
    say(f, (1.0, "final", "second Peter 1 verse 7"))
    acts = say(f, (6.0, "partial", "James a servant of God and of the Lord Jesus Christ"))
    assert [a for a in acts if a.kind == "show"] == []


def test_unrelated_talk_never_moves_it():
    f = Follower(index=chapter_index(), hold=30)
    say(f, (1.0, "final", "second Peter 1 verse 5"))
    acts = say(f, (6.0, "partial", "so what does this mean for us today as a community of faith in this city"))
    assert [a for a in acts if a.kind == "show"] == []


# ---------------------------------------------------------------- confident but assumptive
def voice(f, t, what="speech start"):
    return f.feed({"kind": "status", "t": t, "text": what})


def test_when_the_speaker_carries_on_after_a_finished_verse_the_next_one_goes_up_before_any_word_is_transcribed():
    f = Follower(index=chapter_index(), hold=30)
    say(f, (1.0, "final", "second Peter 1 verse 5"))
    say(f, (4.0, "partial", "for this reason make every effort to supplement your faith with virtue and virtue with knowledge"))
    voice(f, 5.2, "speech end")
    acts = voice(f, 6.0)                                                   # the next sentence begins; no words yet
    assert [(a.reference, a.tentative) for a in acts if a.kind == "show"] == [("2 Peter 1:6", True)]


def test_no_assumption_is_made_when_the_verse_on_screen_was_not_finished():
    f = Follower(index=chapter_index(), hold=30)
    say(f, (1.0, "final", "second Peter 1 verse 5"))
    say(f, (4.0, "partial", "for this reason make every effort to supplement"))
    acts = voice(f, 6.0)
    assert [a for a in acts if a.kind == "show"] == []


def test_no_assumption_is_made_when_the_speaker_never_read_the_verse():
    f = Follower(index=chapter_index(), hold=30)
    say(f, (1.0, "final", "second Peter 1 verse 5"))
    assert [a for a in voice(f, 6.0) if a.kind == "show"] == []


def test_an_assumption_is_confirmed_by_the_first_words_and_stays():
    f = Follower(index=chapter_index(), hold=30)
    say(f, (1.0, "final", "second Peter 1 verse 5"))
    say(f, (4.0, "partial", "for this reason make every effort to supplement your faith with virtue and virtue with knowledge"))
    voice(f, 6.0)
    acts = say(f, (7.5, "partial", "for this reason make every effort to supplement your faith with virtue and virtue with knowledge and knowledge with self control"))
    assert [a for a in acts if a.kind in ("show", "clear")] == [] and f.current.confirmed and f.current.verse == 6


def test_a_wrong_assumption_is_withdrawn_when_the_speaker_moves_on_to_something_else():
    f = Follower(index=chapter_index(), hold=30)
    say(f, (1.0, "final", "second Peter 1 verse 5"))
    say(f, (4.0, "partial", "for this reason make every effort to supplement your faith with virtue and virtue with knowledge"))
    voice(f, 6.0)
    acts = say(f, (9.5, "partial", "for this reason make every effort to supplement your faith with virtue and virtue with knowledge so what does this mean for us today as a church"))
    assert [a.kind for a in acts] == ["clear"] and f.current is None


def test_a_wrong_assumption_returns_to_the_previous_verse_when_the_speaker_was_only_pausing():
    f = Follower(index=chapter_index(), hold=30)
    say(f, (1.0, "final", "second Peter 1 verse 5"))
    say(f, (4.0, "partial", "for this reason make every effort to supplement your faith with virtue and virtue with knowledge"))
    voice(f, 6.0)
    acts = say(f, (9.5, "partial", "for this reason make every effort to supplement your faith with virtue and virtue with knowledge for this reason make every effort to supplement your faith"))
    shows = [a for a in acts if a.kind == "show"]
    assert [a.reference for a in shows] == ["2 Peter 1:5"]


def test_assumptions_never_chain_and_never_pass_the_end_of_a_named_range():
    f = Follower(index=chapter_index(), hold=30)
    say(f, (1.0, "final", "second Peter 1 verses 5 through 6"))
    say(f, (4.0, "partial", "for this reason make every effort to supplement your faith with virtue and virtue with knowledge"))
    assert [a.reference for a in voice(f, 6.0) if a.kind == "show"] == ["2 Peter 1:6"]
    assert [a for a in voice(f, 6.2) if a.kind == "show"] == []             # a guess is not built on a guess
    say(f, (7.5, "partial", "and knowledge with self-control and self-control with steadfastness and steadfastness with godliness"))
    assert [a for a in voice(f, 12.0) if a.kind == "show"] == []             # 6 was the end of the range


def test_the_assumption_can_be_switched_off():
    f = Follower(index=chapter_index(), hold=30, assume=False)
    say(f, (1.0, "final", "second Peter 1 verse 5"))
    say(f, (4.0, "partial", "for this reason make every effort to supplement your faith with virtue and virtue with knowledge"))
    assert [a for a in voice(f, 6.0) if a.kind == "show"] == []


def test_a_blip_of_sound_after_a_finished_verse_is_taken_back_as_soon_as_it_goes_quiet_again():
    f = Follower(index=chapter_index(), hold=30)
    say(f, (1.0, "final", "second Peter 1 verse 5"))
    say(f, (4.0, "partial", "for this reason make every effort to supplement your faith with virtue and virtue with knowledge"))
    guess = [a for a in voice(f, 4.2) if a.kind == "show"]
    assert [a.reference for a in guess] == ["2 Peter 1:6"]
    back = [a for a in voice(f, 5.0, "speech end") if a.kind == "show"]
    assert [a.reference for a in back] == ["2 Peter 1:5"] and f.current.verse == 5


def test_the_next_verse_is_guessed_from_speaking_time_when_the_transcript_is_still_behind():
    f = Follower(index=chapter_index(), hold=30)
    say(f, (1.0, "final", "second Peter 1 verse 5"))
    voice(f, 2.0)                                                           # starts reading verse 5
    say(f, (6.0, "partial", "for this reason make every effort to supplement your faith"))   # transcript is only part-way (about 55%)
    voice(f, 7.6, "speech end")
    acts = voice(f, 8.0)                                                    # but 5.6 s of speech at ~2.7 words/s is the whole verse
    assert [(a.reference, a.tentative) for a in acts if a.kind == "show"] == [("2 Peter 1:6", True)]


def test_a_short_stretch_of_speech_is_not_enough_to_guess_even_if_the_transcript_is_part_way():
    f = Follower(index=chapter_index(), hold=30)
    say(f, (1.0, "final", "second Peter 1 verse 5"))
    voice(f, 2.0)
    say(f, (3.0, "partial", "for this reason make every effort to supplement your faith"))
    voice(f, 3.4, "speech end")                                             # only 1.4 s of speech so far
    assert [a for a in voice(f, 3.8) if a.kind == "show"] == []


def test_a_transcript_that_never_matched_the_verse_cannot_trigger_a_guess_from_timing_alone():
    f = Follower(index=chapter_index(), hold=30)
    say(f, (1.0, "final", "second Peter 1 verse 5"))
    voice(f, 2.0)
    say(f, (7.0, "partial", "so what does this mean for us today as a community of faith"))
    voice(f, 8.0, "speech end")
    assert [a for a in voice(f, 8.5) if a.kind == "show"] == []


# ---------------------------------------------------------------- "Isaiah 42" heard, but Isaiah 40:2 meant
def isaiah_index():
    v = {
        (40, 1): "Comfort, comfort my people, says your God.",
        (40, 2): "Speak tenderly to Jerusalem, and cry to her that her warfare is ended, that her iniquity is pardoned, that she has received from the Lord's hand double for all her sins.",
        (42, 1): "Behold my servant, whom I uphold, my chosen, in whom my soul delights; I have put my Spirit upon him; he will bring forth justice to the nations.",
        (4, 2): "In that day the branch of the Lord shall be beautiful and glorious, and the fruit of the land shall be the pride and honor of the survivors of Israel.",
    }
    return ScriptureIndex({"ESV": [Verse("ISA", "Isaiah", c, n, x) for (c, n), x in v.items()]})


def test_a_chapter_with_no_verse_is_re_read_as_chapter_and_verse_when_that_verse_follows():
    f = Follower(index=isaiah_index(), hold=30)
    assert [a for a in say(f, (1.0, "partial", "turn with me to Isaiah 42")) if a.kind == "show"] == []
    acts = say(f, (3.5, "partial", "turn with me to Isaiah 42 speak tenderly to Jerusalem and cry to her that her warfare is ended"))
    shows = [a for a in acts if a.kind == "show"]
    assert [a.reference for a in shows] == ["Isaiah 40:2"] and "only a chapter" in shows[0].reason


def test_it_waits_for_the_pause_before_re_reading_a_chapter():
    f = Follower(index=isaiah_index(), hold=30)
    acts = say(f, (1.0, "partial", "Isaiah 42 speak tenderly to Jerusalem and cry to her that her warfare is ended"))
    assert [a for a in acts if a.kind == "show"] == []                  # no pause yet: the words may be running on
    acts = say(f, (2.6, "partial", "Isaiah 42 speak tenderly to Jerusalem and cry to her that her warfare is ended"))
    assert [a.reference for a in acts if a.kind == "show"] == ["Isaiah 40:2"]


def test_a_chapter_that_really_was_the_chapter_is_found_by_its_first_verse():
    f = Follower(index=isaiah_index(), hold=30)
    say(f, (1.0, "partial", "Isaiah 42"))
    acts = say(f, (4.0, "partial", "Isaiah 42 behold my servant whom I uphold my chosen in whom my soul delights"))
    assert [a.reference for a in acts if a.kind == "show"] == ["Isaiah 42:1"]


def test_a_chapter_with_nothing_matching_after_it_shows_nothing():
    f = Follower(index=isaiah_index(), hold=30)
    say(f, (1.0, "partial", "Isaiah 42"))
    acts = say(f, (4.0, "partial", "Isaiah 42 so what does this mean for us as a community of faith today"))
    assert [a for a in acts if a.kind == "show"] == []


def test_a_full_reference_is_never_second_guessed():
    f = Follower(index=isaiah_index(), hold=30)
    acts = say(f, (1.0, "final", "Isaiah 40 2"), (4.0, "partial", "Isaiah 40 2 speak tenderly to Jerusalem and cry to her that her warfare is ended"))
    assert [a.reference for a in acts if a.kind == "show"] == ["Isaiah 40:2"]


def test_the_chapter_re_read_is_dropped_if_the_verse_never_starts():
    f = Follower(index=isaiah_index(), hold=30)
    say(f, (1.0, "partial", "Isaiah 42"))
    acts = say(f, (30.0, "partial", "speak tenderly to Jerusalem and cry to her that her warfare is ended"))
    assert [a.reference for a in acts if a.reason.startswith("heard only a chapter")] == []


def james_index():
    v = {2: "Count it all joy, my brothers, when you meet trials of various kinds,",
         3: "for you know that the testing of your faith produces steadfastness.",
         4: "And let steadfastness have its full effect, that you may be perfect and complete, lacking in nothing."}
    return ScriptureIndex({"ESV": [Verse("JAS", "James", 1, n, x) for n, x in v.items()]})


def test_a_guess_is_kept_when_its_first_words_are_garbled_but_the_verse_then_comes_out_right():
    f = Follower(index=james_index(), hold=30)
    say(f, (1.0, "final", "James 1 verse 2"))
    say(f, (4.0, "partial", "count it all joy my brothers when you meet trials of various kinds"))
    assert [a.reference for a in voice(f, 4.6) if a.kind == "show"] == ["James 1:3"]
    acts = say(f, (5.5, "final", "when you meet trials of various kinds"),
               (6.8, "partial", 'the bible says for the most glorious kinds'),
               (7.6, "partial", "the"),
               (9.6, "partial", "for you know that the testing of your faith produces steadfastness"))
    assert [a for a in acts if a.kind in ("show", "clear")] == [] and f.current.verse == 3 and f.current.confirmed


def test_a_guess_is_not_withdrawn_in_its_first_seconds_on_garbled_words_alone():
    f = Follower(index=james_index(), hold=30)
    say(f, (1.0, "final", "James 1 verse 2"))
    say(f, (4.0, "partial", "count it all joy my brothers when you meet trials of various kinds"))
    voice(f, 4.6)
    acts = say(f, (5.8, "partial", "so and the bible says for the most glorious kinds of"))
    assert [a for a in acts if a.kind in ("show", "clear")] == [] and f.current.verse == 3
