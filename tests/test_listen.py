from versevision.listen import Detector, run
from versevision.references.spoken import find_spoken_references


def refs(text):
    return [r.text for r in find_spoken_references(text)]


def test_spoken_forms_become_written_references():
    assert refs("turn with me to John chapter three verse sixteen") == ["John 3:16"]
    assert refs("first Corinthians thirteen four") == ["1 Corinthians 13:4"]
    assert refs("Romans eight twenty eight") == ["Romans 8:28"]
    assert refs("Psalm twenty three one through three") == ["Psalms 23:1-3"]
    assert refs("Psalm one nineteen one oh five") == ["Psalms 119:105"]
    assert refs("Jude twenty four") == ["Jude 1:24"]
    assert refs("Ephesians two eight through nine") == ["Ephesians 2:8-9"]
    assert refs("John 3:16") == ["John 3:16"]


def test_ordinary_talk_never_triggers():
    for line in ["we have ten minutes and chapter three is next", "in the second chapter he said",
                 "the first time I read Acts it was two thousand", "verse sixteen is the key", "chapter three"]:
        assert refs(line) == [], line


def test_two_references_in_one_breath_keep_their_order():
    assert refs("John three sixteen and then Romans eight twenty eight") == ["John 3:16", "Romans 8:28"]


def stream(*items):
    return [{"kind": k, "t": t, "text": x} for k, t, x in items]


def test_a_reference_is_acted_on_once_after_it_settles_even_though_partials_repeat():
    shown = []
    found = run(stream(("partial", 1.0, "turn to John"), ("partial", 1.3, "turn to John three"), ("partial", 1.6, "turn to John three sixteen"),
                       ("partial", 2.0, "turn to John three sixteen and"), ("partial", 2.4, "turn to John three sixteen and we see"),
                       ("final", 3.0, "turn to John three sixteen and we see")),
                show=lambda r: shown.append(r) or True, log=lambda s: None)
    assert shown == ["John 3:16"] and [d.action for d in found] == ["shown"]


def test_a_half_heard_reference_that_changes_is_not_shown_early():
    shown = []
    run(stream(("partial", 1.0, "John three"), ("partial", 1.2, "John three sixteen"), ("partial", 1.4, "John three sixteen"),
               ("partial", 2.1, "John three sixteen"), ("final", 2.2, "John three sixteen")),
        show=lambda r: shown.append(r) or True, log=lambda s: None)
    assert shown == ["John 3:16"]            # not "John 3:1" or twice


def test_the_same_reference_inside_the_cooldown_is_skipped_but_a_later_one_is_shown():
    shown = []
    found = run(stream(("final", 1.0, "John three sixteen"), ("final", 10.0, "again John three sixteen"), ("final", 50.0, "John three sixteen once more")),
                show=lambda r: shown.append(r) or True, detector=Detector(cooldown=30), log=lambda s: None)
    assert shown == ["John 3:16", "John 3:16"] and [d.action for d in found] == ["shown", "skipped-repeat", "shown"]


def test_dry_run_shows_nothing():
    found = run(stream(("final", 1.0, "Romans eight twenty eight")), show=None, log=lambda s: None)
    assert [d.reference for d in found] == ["Romans 8:28"] and found[0].action == "logged"


def test_a_failed_show_is_reported_not_hidden():
    found = run(stream(("final", 1.0, "Romans eight twenty eight")), show=lambda r: False, log=lambda s: None)
    assert found[0].action == "show-failed"


def test_what_apple_speech_recognition_really_writes_is_understood():
    """Captured from the iMac: numbers come back as digits, and 'eight twenty eight' arrives as 828."""
    heard = "Turn with me to John chapter 3 verse 16 now look at Romans 828 and then first Corinthians 13 four Psalm 119 105"
    assert refs(heard) == ["John 3:16", "Romans 8:28", "1 Corinthians 13:4", "Psalms 119:105"]
    assert refs("Luke 1423") == ["Luke 14:23"]
    assert refs("Mark 16 15") == ["Mark 16:15"]


def test_ambiguous_run_together_digits_are_not_guessed():
    assert refs("Hebrews 1131") == []         # 11:31 or 1:131? do not guess in front of a congregation


def test_a_reference_is_acted_on_even_if_the_recogniser_goes_quiet_without_a_final_result():
    """Seen on the iMac: 'John chapter 3 verse 16' was the last partial and no final ever came."""
    shown = []
    run(stream(("partial", 9.85, "please turn with me to John chapter 3"), ("partial", 10.75, "please turn with me to John chapter 3 verse 16"),
               ("partial", 14.14, "Call"), ("partial", 15.25, "Paul says in Romans eight")),
        show=lambda r: shown.append(r) or True, log=lambda s: None)
    assert shown == ["John 3:16"]


def test_a_new_sentence_can_repeat_a_reference_after_the_cooldown():
    shown = []
    run(stream(("partial", 1.0, "John three sixteen"), ("status", 3.0, "x"), ("partial", 40.0, "Back to John three sixteen"), ("status", 42.0, "x")),
        show=lambda r: shown.append(r) or True, detector=Detector(cooldown=30), log=lambda s: None)
    assert shown == ["John 3:16", "John 3:16"]


def test_a_number_the_recogniser_is_still_revising_is_not_shown():
    """Seen live: 'Romans 820' appeared first, then became 'Romans 828'. The wrong verse must never be shown."""
    shown = []
    run(stream(("partial", 15.25, "Paul says in Romans eight"), ("partial", 15.54, "Paul says in Romans 820"),
               ("status", 16.5, "tick"), ("partial", 16.7, "Paul says in Romans 828"), ("partial", 17.0, "Paul says in Romans 828 that all"),
               ("partial", 18.0, "Paul says in Romans 828 that all things work")),
        show=lambda r: shown.append(r) or True, log=lambda s: None)
    assert shown == ["Romans 8:28"]


def test_a_reference_at_the_very_end_of_speech_is_shown_after_a_pause():
    shown = []
    run(stream(("partial", 10.0, "turn to John 316"), ("status", 11.0, "tick"), ("status", 12.0, "tick"), ("status", 12.5, "tick")),
        show=lambda r: shown.append(r) or True, log=lambda s: None)
    assert shown == ["John 3:16"]
