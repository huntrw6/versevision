from __future__ import annotations

from pydantic import BaseModel, Field

from versevision import runlog
from versevision.bibles import ProviderRegistry
from versevision.config import AppConfig, PointsConfig, VersesConfig
from versevision.errors import VerseVisionError
from versevision.notes import NoteDocument, PointItem, classify_document
from versevision.propresenter.api_client import ProPresenterApiClient
from versevision.propresenter.selection import select_unique_presentation
from versevision.references.models import PassageReference
from versevision.slides.models import PlannedSlide, SlidePlan
from versevision.slides.planner import placeholder_passages, plan_slides
from versevision.versions.registry import canonical_version
from .result import TargetResult


_NUMBERED = __import__("re").compile(r"^(#?\d+[).:]|[A-Za-z][).]|[A-Za-z]{2,}\s*#?\s*\d+[).:\-])$")      # "1)", "#2:", "a)", "KEY #3)"


def _title_case(text: str) -> str:
    small = {"a", "an", "and", "as", "at", "but", "by", "for", "in", "of", "on", "or", "the", "to", "with"}
    words = text.split(" ")
    return " ".join(w if (0 < i < len(words) - 1 and w.lower() in small) else (w[:1].upper() + w[1:].lower() if w.islower() or w.isupper() else w) for i, w in enumerate(words))


def format_point(item: PointItem, cfg: PointsConfig, counter: int) -> str:
    """The text of a point slide: its label (kept, dropped, renumbered, or the bullet mark) and the case of the words."""
    label = item.label
    if _NUMBERED.match(label or ""):
        if cfg.numbering == "strip":
            label = ""
        elif cfg.numbering == "renumber" and item.role == "point":
            word = __import__("re").match(r"^([A-Za-z]{2,})", label)
            label = (f"{word.group(1)} #{counter}{label[-1]}" if word else cfg.number_style.format(n=counter))
    elif label == "•":
        label = cfg.bullet_char
    text = item.text
    if cfg.case == "upper":
        text = text.upper()
    elif cfg.case == "lower":
        text = text.lower()
    elif cfg.case == "title":
        text = "\n".join(_title_case(line) for line in text.split("\n"))
    return f"{label} {text}".strip()


def added_point_text(text: str, cfg: PointsConfig) -> str:
    """A line you chose to add as a slide in the review: formatted like any other plain point."""
    return format_point(PointItem(line=0, role="list_item", label="•", text=" ".join(text.split())), cfg, 0)


def apply_review_choices(result: "NotesImportResult", skip: set[int] = frozenset(), include: set[int] = frozenset()) -> SlidePlan:
    """The slides after your choices in the review. ``skip``: decisions whose slides you switched off. ``include``: left-out lines
    you chose to add as a slide; each goes where the line stood in the document. Ids are the decision ids in ``result.decisions``."""
    from versevision import runlog
    slides = list(result.slide_plan.slides)
    drop = {n for d in result.decisions if d.id in skip for n in range(d.first_slide, d.first_slide + d.slide_count)}
    inserts: dict[int, list[PlannedSlide]] = {}
    added = []
    for d in result.decisions:
        if d.id in include and d.slide_count == 0 and d.add_text:
            inserts.setdefault(min(d.first_slide, len(slides)), []).append(PlannedSlide(
                kind="point", body_text=d.add_text, reference_text="", version_text="", source_passage_index=100_000 + d.id))
            added.append({"id": d.id, "line": d.line, "text": d.add_text, "at_slide": min(d.first_slide, len(slides)) + 1})
    out: list[PlannedSlide] = []
    for index in range(len(slides) + 1):
        out.extend(inserts.get(index, []))
        if index < len(slides) and index not in drop:
            out.append(slides[index])
    if added:
        runlog.trace("review_included", added=added, note="left-out lines you chose to add as slides in the review (at the place they stood in the document)")
    return SlidePlan(slides=out, looks=result.slide_plan.looks)


class ProblemEntry(BaseModel):
    line: int
    text: str
    problem: str


class Decision(BaseModel):
    """One thing the importer decided about one part of the notes, with how sure it was.

    tier "good"    parsed and formatted with no judgment calls (very likely right)
    tier "warning" identified, but a rule that is partly a guess was used (look at it)
    tier "error"   could not be decided (nothing is added for it until you fix it)
    """
    id: int
    line: int
    kind: str                      # verse | point | left_out | error
    title: str
    text: str = ""
    tier: str = "good"
    reasons: list[str] = Field(default_factory=list)
    first_slide: int = 0           # index of the first slide it produced in the plan (0-based); for a decision with no slide, where one would go
    slide_count: int = 0
    add_text: str = ""             # for a left-out line: the slide text it would make if you choose "add this as a slide" in the review


_FIRM_LEFT_OUT = ("stage/production cue", "title / date line", "recap heading", "document title (first line)", "email header line",
                  "verse text quoted above its reference", "title / heading line", "[slide] tag that points")


def _left_out_tier(reason: str) -> str:
    return "good" if reason.startswith(_FIRM_LEFT_OUT) or any(f in reason for f in _FIRM_LEFT_OUT) else "warning"


class NotesImportResult(BaseModel):
    slide_plan: SlidePlan = Field(default_factory=SlidePlan)
    verse_count: int = 0
    point_count: int = 0
    skipped: list[ProblemEntry] = Field(default_factory=list)     # intentionally left out
    review: list[ProblemEntry] = Field(default_factory=list)      # unclear lines, need a decision
    failed: list[ProblemEntry] = Field(default_factory=list)      # wanted but could not be added
    notices: list[ProblemEntry] = Field(default_factory=list)     # corrections the tool made
    decisions: list[Decision] = Field(default_factory=list)       # every decision, in document order, with its confidence
    target: TargetResult | None = None
    fetched: bool = False
    files_changed: bool = False

    @property
    def needs_attention(self) -> bool:
        return bool(self.review or self.failed)


def _words(text: str) -> list[str]:
    import re as _re
    return _re.sub(r"[^a-z0-9' ]", " ", text.lower().replace("’", "'").replace("‘", "'")).split()


def quote_score(quoted: str, passage) -> float:
    """How much of the quoted words appear, in order, in a version's wording of the passage (0 to 1). Verse numbers, punctuation and case do not count."""
    import difflib
    wanted = _words(quoted)
    have = _words(" ".join(v.text for v in passage.verses))
    if not wanted or not have:
        return 0.0
    matched = sum(block.size for block in difflib.SequenceMatcher(None, wanted, have, autojunk=False).get_matching_blocks())
    return matched / len(wanted)          # how much of the QUOTE is in the verse (a quote may be only part of it)


async def check_version_against_quote(item, reference, passage, registry, config, reasons: list[str]):
    """The notes quote this verse and did not name a version. If the quote does not match the version it was given (the version above or the
    default) but does match another version's wording, use that version. Returns (reference, passage)."""
    cfg = config.verses
    if cfg.version_from_quote != "use" or not item.quoted or item.reference.version or registry is None:
        return reference, passage
    current = reference.version or ""
    base = quote_score(item.quoted, passage)
    tried = {current.casefold(): round(base, 2)}
    if base >= cfg.quote_match:
        runlog.trace("quote_check", line=item.line, reference=item.reference.normalized_text, quoted=item.quoted, version=current, score=round(base, 2), result="the quote matches this version")
        return reference, passage
    best = (0.0, None, None)
    for name in cfg.quote_versions:
        canon = canonical_version(name)
        if not canon or canon.casefold() == current.casefold():
            continue
        try:
            trial_ref = reference.model_copy(update={"version": canon})
            trial = await registry.fetch(trial_ref)
        except Exception:
            continue
        score = quote_score(item.quoted, trial)
        tried[canon.casefold()] = round(score, 2)
        if score > best[0]:
            best = (score, trial_ref, trial)
        if score >= 0.95:
            break
    if best[1] is not None and best[0] >= cfg.quote_match and best[0] >= base + 0.1:
        runlog.trace("quote_check", line=item.line, reference=item.reference.normalized_text, quoted=item.quoted, version=current, tried=tried,
                     result=f"the notes quote {best[1].version} wording (score {best[0]:.2f} vs {base:.2f} for {current}): used {best[1].version}")
        reasons[:] = [r for r in reasons if not r.startswith("no version listed")]       # superseded: the version came from the quote
        reasons.append(f"no version listed; the quoted text matches {best[1].version}, not {current} (used {best[1].version}; turn off with verses.version_from_quote=off)")
        return best[1], best[2]
    runlog.trace("quote_check", line=item.line, reference=item.reference.normalized_text, quoted=item.quoted, version=current, tried=tried,
                 result="the quote matches none of the versions tried; kept " + current)
    if base < 0.6:
        reasons.append(f"the quoted text in the notes does not match {current} wording (match {base:.0%}); check the version")
    return reference, passage


def _trace_verse(item, reference, passage, planned, config) -> None:
    """What the Bible provider returned for one verse entry and how it became slides."""
    from versevision.slides.fitting import wrap_body_lines
    verses = passage.verses
    runlog.trace("fetch_result", line=item.line, reference=reference.normalized_text, version_asked=reference.version, version_returned=passage.resolved_version,
                 verses=len(verses), characters=sum(len(v.text) for v in verses), first_verse=(verses[0].text[:100] if verses else ""),
                 last_verse=(verses[-1].text[-100:] if verses else ""), copyright=next((v.copyright or v.attribution for v in verses if (v.copyright or v.attribution)), None),
                 note="which provider answered is in events.jsonl (step bible_fetch)")
    per_verse = []
    for v in verses:
        mine = [x for x in planned if x.verse_keys == [v.key]]
        body = f"{v.verse} {v.text}".rstrip() if config.slides.include_verse_numbers else v.text
        per_verse.append({"verse": v.key, "characters": len(body), "wrapped_lines": len(wrap_body_lines(" ".join(body.split()), config.slides.line_width_units)),
                          "slides": len(mine), "pieces": [x.body_text for x in mine]})
    runlog.trace("split", line=item.line, reference=reference.normalized_text, slides=len(planned),
                 settings={"max_body_lines": config.slides.max_body_lines, "line_width_units": config.slides.line_width_units,
                           "prefer_punctuation_breaks": config.slides.prefer_punctuation_breaks, "include_verse_numbers": config.slides.include_verse_numbers,
                           "break_on_new_verse": config.slides.break_on_new_verse, "reference_label_mode": config.slides.reference_label_mode},
                 verses=per_verse,
                 note="a verse is cut into several slides only when it needs more wrapped lines than max_body_lines; each cut prefers a punctuation break near the middle")


def resolve_note_versions(
    document: NoteDocument, *, default: str | None, source: str = "above"
) -> tuple[list[tuple[int, PassageReference | None, str]], list[ProblemEntry]]:
    """Give every verse a version: its own, else the nearest explicit one above, else the default."""
    notices: list[ProblemEntry] = []
    resolved: list[tuple[int, PassageReference | None, str]] = []
    last_explicit: str | None = None
    default_name = canonical_version(default) if default else None
    verses = document.verses

    def same_passage(a: PassageReference, b: PassageReference) -> bool:
        return a.book_id == b.book_id and a.spans == b.spans

    def comparison(index: int) -> bool:
        """Same passage shown in two versions back to back (a study comparison)."""
        here = verses[index].reference
        return any(
            0 <= other < len(verses) and same_passage(here, verses[other].reference)
            and verses[other].reference.version != here.version
            for other in (index - 1, index + 1)
        )

    last_explicit_line = 0
    for index, item in enumerate(verses):
        reference = item.reference
        for warning in item.warnings:
            if "read as" in warning or "unrecognized" in warning:
                notices.append(ProblemEntry(line=item.line, text=reference.source_text.strip(), problem=warning + (
                    "; ignored, so the verse uses the nearest version above or the default" if "unrecognized" in warning else "")))
        if reference.version:
            pair = comparison(index)
            if not pair:          # a comparison pair never sets the version for later verses
                last_explicit, last_explicit_line = reference.version, item.line
            runlog.trace("version", line=item.line, reference=reference.normalized_text, version=reference.version, how="written on the verse",
                         sets_version_for_later_verses=not pair,
                         note="same passage appears next to it in another version: a comparison pair, so it does not set the version for later verses" if pair else "")
            resolved.append((item.line, reference, ""))
            continue
        if source == "required":
            runlog.trace("version", line=item.line, reference=reference.normalized_text, version=None, how="none written and verses.version_source=required: not added")
            resolved.append((item.line, None, "no version on this verse, and your setting requires one (verses.version_source = required)"))
            continue
        chosen = (last_explicit or default_name) if source == "above" else default_name
        runlog.trace("version", line=item.line, reference=reference.normalized_text, version=chosen,
                     how=(f"none written; used the nearest version above (line {last_explicit_line})" if (last_explicit and source == "above")
                          else "none written; used the default version"), version_source=source,
                     default_version=default_name, last_explicit_version=last_explicit)
        if not chosen:
            resolved.append((item.line, None, "no version on this verse, none above it, and no default version is set" if source == "above" else "no version on this verse and no default version is set"))
            continue
        how = "nearest version above" if (last_explicit and source == "above") else "default version"
        notices.append(ProblemEntry(line=item.line, text=reference.normalized_text, problem=f"no version listed; used {chosen} ({how})"))
        resolved.append((item.line, reference.model_copy(update={"version": chosen}), ""))
    return resolved, notices


async def plan_notes(
    source: str,
    config: AppConfig,
    *,
    default_version: str | None = None,
    no_fetch: bool = False,
    target_uuid: str | None = None,
    target_name: str | None = None,
    library: str | None = None,
    provider_registry: ProviderRegistry | None = None,
) -> NotesImportResult:
    run = runlog.current()
    if run:
        run.save_options({"notes": config.notes.model_dump(mode="json"), "points": config.points.model_dump(mode="json"),
                          "slides": config.slides.model_dump(mode="json"), "verses": config.verses.model_dump(mode="json"),
                          "bible": {"default_version": config.bible.default_version, "version_aliases": config.bible.version_aliases,
                                    "version_routes": config.bible.version_routes},
                          "default_version_used": default_version or config.bible.default_version, "no_fetch": no_fetch})
    document = classify_document(source, config.bible.version_aliases, config.notes, config.points)
    result = NotesImportResult(fetched=not no_fetch)
    for item in document.skipped:
        result.skipped.append(ProblemEntry(line=item.line, text=item.text, problem=f"{item.category}: {item.reason}"))
    for item in document.review:
        result.review.append(ProblemEntry(line=item.line, text=item.text, problem=item.reason))
    for issue in document.issues:
        result.failed.append(ProblemEntry(line=0, text=issue.source_text, problem=f"invalid reference: {issue.message}"))

    default = default_version or config.bible.default_version
    if default and not canonical_version(default):
        raise VerseVisionError(f"Unknown Bible version: {default}")
    versions, notices = resolve_note_versions(document, default=default, source=config.verses.version_source)
    result.notices = notices
    reasons_by_line: dict[int, list[str]] = {}
    for n in notices:
        reasons_by_line.setdefault(n.line, []).append(n.problem)
    resolved_refs = {id(item): entry for item, entry in zip(document.verses, versions)}

    registry = provider_registry
    owns = False
    if not no_fetch and registry is None:
        registry = ProviderRegistry(config.bible)
        owns = True
    slides: list[PlannedSlide] = []
    group_index = 0
    numbered_points = 0
    try:
        def add(item, kind: str, title: str, text: str, tier: str, reasons: list[str], first: int = 0, count: int = 0) -> None:
            seen: list[str] = []
            for r in reasons:
                if r and r not in seen:
                    seen.append(r)
            extra = {}
            if count == 0 and kind in ("left_out", "error") and getattr(item, "kind", "") in ("skipped", "review") and text.strip():
                extra = {"first_slide": len(slides), "add_text": added_point_text(text, config.points)}
            result.decisions.append(Decision(id=len(result.decisions), line=item.line, kind=kind, title=title, text=text, tier=tier,
                                             reasons=seen, **({"first_slide": first, "slide_count": count} | extra)))

        for item in document.items:
            if item.kind == "point":
                assert isinstance(item, PointItem)
                first = len(slides)
                if item.role == "point" and item.label and _NUMBERED.match(item.label):
                    numbered_points += 1
                shown_text = format_point(item, config.points, numbered_points)
                slides.append(PlannedSlide(
                    kind="point", body_text=shown_text, reference_text="", version_text="",
                    source_passage_index=group_index,
                ))
                group_index += 1
                result.point_count += 1
                for warning in item.warnings:
                    result.notices.append(ProblemEntry(line=item.line, text=item.display[:60], problem=warning))
                runlog.trace("point_format", line=item.line, role=item.role, label_in_notes=item.label, text_in_notes=item.text, slide_text=shown_text,
                             settings={"numbering": config.points.numbering, "case": config.points.case, "bullet_char": config.points.bullet_char,
                                       "number_style": config.points.number_style})
                add(item, "point", shown_text.split("\n")[0][:80], shown_text, "warning" if item.warnings else "good", list(item.warnings), first, 1)
            elif item.kind == "verse":
                line, reference, why = resolved_refs[id(item)]
                shown = item.reference.normalized_text
                extra = list(reasons_by_line.get(item.line, [])) + [w for w in item.warnings if w not in reasons_by_line.get(item.line, [])]
                if reference is None:
                    result.failed.append(ProblemEntry(line=line, text=shown, problem=why))
                    add(item, "error", shown, item.reference.source_text.strip(), "error", [why] + extra)
                    continue
                try:
                    passage = (await registry.fetch(reference)) if registry else placeholder_passages([reference])[0]
                    if not passage.verses:
                        raise VerseVisionError("provider returned no verses")
                    if registry and not no_fetch:
                        reference, passage = await check_version_against_quote(item, reference, passage, registry, config, extra)
                except Exception as exc:  # one unavailable verse never stops the rest
                    inherited = reference.version != item.reference.version
                    hint = f" (version {reference.version} was taken from the verse above; add a version to this verse to change it)" if inherited else ""
                    result.failed.append(ProblemEntry(
                        line=line, text=reference.normalized_text,
                        problem=f"not added: {exc}{hint}",
                    ))
                    add(item, "error", reference.normalized_text, item.reference.source_text.strip(), "error", [f"not added: {exc}{hint}"])
                    continue
                first = len(slides)
                planned = plan_slides([passage], config.slides).slides
                _trace_verse(item, reference, passage, planned, config)
                for slide in planned:
                    slide.source_passage_index = group_index
                slides.extend(planned)
                group_index += 1
                result.verse_count += 1
                norm = reference.normalized_text
                label = norm if (not reference.version or reference.version.casefold() in norm.casefold()) else f"{norm} ({reference.version})"
                add(item, "verse", label, item.reference.source_text.strip(), "warning" if extra else "good", extra, first, len(planned))
            elif item.kind == "review":
                add(item, "error", item.text[:80], item.text, "error", [item.reason])
            elif item.kind == "skipped":
                tier = _left_out_tier(item.reason)
                add(item, "left_out", item.text[:80], item.text, tier, [f"{item.category.replace('_', ' ')}: {item.reason}"] if tier == "warning" else [item.reason])
        for issue in document.issues:
            result.decisions.append(Decision(id=len(result.decisions), line=0, kind="error", title=issue.source_text[:80], text=issue.source_text,
                                             tier="error", reasons=[f"invalid reference: {issue.message}"]))
    finally:
        if owns and registry:
            await registry.aclose()
    result.slide_plan = SlidePlan(slides=slides)
    for d in result.decisions:
        runlog.trace("decision", id=d.id, line=d.line, kind=d.kind, tier=d.tier, title=d.title, reasons=d.reasons,
                     slides=[n + 1 for n in range(d.first_slide, d.first_slide + d.slide_count)])
    runlog.trace("plan", slide_count=len(slides), verse_slides=sum(1 for x in slides if x.kind == "verse"), point_slides=sum(1 for x in slides if x.kind == "point"),
                 slides=[{"n": i + 1, "kind": x.kind, "reference": x.reference_text, "version": x.version_text, "text": x.body_text,
                          "verses": x.verse_keys} for i, x in enumerate(slides)])

    if target_uuid or target_name:
        pp = config.propresenter
        async with ProPresenterApiClient(pp.host, pp.port, pp.timeout_seconds) as client:
            selected = select_unique_presentation(await client.presentations(), uuid=target_uuid, name=target_name, library=library)
            metadata = await client.presentation(selected.uuid)
            result.target = TargetResult(
                library_name=selected.library_name or "", presentation_name=selected.name,
                uuid=selected.uuid, path=metadata.presentation_path,
            )
    return result
