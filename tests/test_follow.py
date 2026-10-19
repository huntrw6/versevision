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
    assert all("began reading" in g[1] for g in got[1:])


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
