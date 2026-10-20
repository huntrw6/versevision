"""Replay a listening log through the Follower and score the screen against what was actually being read."""
import json, sys, time
from versevision.follow import Follower
from versevision.scripture import ScriptureIndex, normalize
idx = ScriptureIndex.load()

def truth_verse(book, chap, text, nverses):
    words = normalize(text).split()[-10:]
    if len(words) < 5: return None
    best = None
    for v in range(1, nverses + 1):
        for vn in idx.verse_texts(book, chap, v):
            m_, cov, last, first = idx.tail_fit(words, vn.split())
            if m_ >= 5 and cov >= 0.75 and last and first <= max(8, len(vn.split()) // 2):
                sc = m_ * cov
                if best is None or sc > best[0]: best = (sc, v)
    return best[1] if best else None

def run(path, book, chap, hold=30):
    rows = [json.loads(l) for l in open(path)]
    nverses = max(v for (b, c, v) in idx.by_ref if b == book and c == chap)
    f = Follower(index=idx, hold=hold)
    screen = None; events = []; seen = {}
    t0 = rows[0]["w"]
    stats = dict(rows=0, ok=0, late=0, early=0, blank=0)
    first_truth = {}; first_screen = {}
    show_times = []
    for r in rows:
        if r["kind"] not in ("partial", "final", "status"): continue
        for a in f.feed(r):
            if a.kind == "show":
                screen = a.verse if (a.book_id == book and a.chapter == chap) else -1
                events.append((r["w"] - t0, "show", a.reference, a.reason[:34])); show_times.append((r["w"] - t0, a.verse))
                first_screen.setdefault(a.verse, r["w"] - t0)
            else:
                screen = None; events.append((r["w"] - t0, "clear", "", a.reason[:34]))
        if r["kind"] in ("partial", "final"):
            tv = truth_verse(book, chap, r["text"], nverses)
            if tv is None: continue
            first_truth.setdefault(tv, r["w"] - t0)
            stats["rows"] += 1
            if screen == tv: stats["ok"] += 1
            elif screen is None: stats["blank"] += 1
            elif screen is not None and screen > tv: stats["early"] += 1
            else: stats["late"] += 1
    # flickers: a show replaced or cleared within 5 s
    flick = 0
    for i, e in enumerate(events):
        if e[1] == "show" and i + 1 < len(events) and events[i + 1][0] - e[0] < 5.0: flick += 1
    lat = []
    for v, t in sorted(first_truth.items()):
        s = first_screen.get(v)
        lat.append((v, None if s is None else round(s - t, 1)))
    return stats, flick, lat, events

if __name__ == "__main__":
    path, book, chap = sys.argv[1], sys.argv[2], int(sys.argv[3])
    stats, flick, lat, events = run(path, book, chap)
    n = max(1, stats["rows"])
    print("rows=%d  on-screen-correct=%d%%  late=%d%%  early(premature)=%d%%  blank=%d%%  flickers=%d" % (n, 100*stats["ok"]/n, 100*stats["late"]/n, 100*stats["early"]/n, 100*stats["blank"]/n, flick))
    print("verse: seconds from first being identified in speech to on screen (negative = was up before the transcript showed it); None = never shown")
    print("  " + "  ".join("v%d:%s" % x for x in lat))
    if len(sys.argv) > 4:
        for e in events: print("  %6.1fs %-5s %-16s %s" % e)

def detail(path, book, chap):
    rows = [json.loads(l) for l in open(path)]
    nverses = max(v for (b, c, v) in idx.by_ref if b == book and c == chap)
    f = Follower(index=idx, hold=30)
    screen = None; t0 = rows[0]["w"]
    for r in rows:
        if r["kind"] not in ("partial", "final", "status"): continue
        for a in f.feed(r):
            screen = a.verse if a.kind == "show" else None
            print("   %6.1fs %s %s" % (r["w"] - t0, "SHOW " + str(a.verse) if a.kind == "show" else "CLEAR", a.reason[:50]))
        if r["kind"] in ("partial", "final"):
            tv = truth_verse(book, chap, r["text"], nverses)
            if tv is not None and screen is not None and screen != tv:
                print("   %6.1fs   screen=%s truth=%s | %s" % (r["w"] - t0, screen, tv, r["text"][-70:]))
