from dataclasses import dataclass
import re


@dataclass(frozen=True)
class Book:
    id: str
    name: str
    chapters: int
    aliases: tuple[str, ...] = ()


_DATA = [
 ("GEN","Genesis",50,("Gen",)),("EXO","Exodus",40,("Exod","Ex")),("LEV","Leviticus",27,("Lev",)),("NUM","Numbers",36,("Num",)),("DEU","Deuteronomy",34,("Deut","Dt")),
 ("JOS","Joshua",24,("Josh",)),("JDG","Judges",21,("Judg","Jdgs")),("RUT","Ruth",4,()),("1SA","1 Samuel",31,("1 Sam","I Samuel","First Samuel")),("2SA","2 Samuel",24,("2 Sam","II Samuel","Second Samuel")),
 ("1KI","1 Kings",22,("1 Kgs","I Kings","First Kings")),("2KI","2 Kings",25,("2 Kgs","II Kings","Second Kings")),("1CH","1 Chronicles",29,("1 Chron","I Chronicles","First Chronicles")),("2CH","2 Chronicles",36,("2 Chron","II Chronicles","Second Chronicles")),
 ("EZR","Ezra",10,()),("NEH","Nehemiah",13,("Neh",)),("EST","Esther",10,("Esth",)),("JOB","Job",42,()),("PSA","Psalms",150,("Psalm","Ps","Psa")),("PRO","Proverbs",31,("Prov","Pr")),("ECC","Ecclesiastes",12,("Eccl","Ecc")),
 ("SNG","Song of Solomon",8,("Song of Songs","Song","SOS","Canticles")),("ISA","Isaiah",66,("Isa",)),("JER","Jeremiah",52,("Jer",)),("LAM","Lamentations",5,("Lam",)),("EZK","Ezekiel",48,("Ezek",)),("DAN","Daniel",12,("Dan",)),
 ("HOS","Hosea",14,("Hos",)),("JOL","Joel",3,()),("AMO","Amos",9,()),("OBA","Obadiah",1,("Obad",)),("JON","Jonah",4,("Jon",)),("MIC","Micah",7,("Mic",)),("NAM","Nahum",3,("Nah",)),("HAB","Habakkuk",3,("Hab",)),
 ("ZEP","Zephaniah",3,("Zeph",)),("HAG","Haggai",2,("Hag",)),("ZEC","Zechariah",14,("Zech",)),("MAL","Malachi",4,("Mal",)),("MAT","Matthew",28,("Matt","Mt")),("MRK","Mark",16,("Mk","Mrk")),("LUK","Luke",24,("Lk",)),
 ("JHN","John",21,("Jn","Jhn")),("ACT","Acts",28,("Ac",)),("ROM","Romans",16,("Rom",)),("1CO","1 Corinthians",16,("1 Cor","I Corinthians","First Corinthians")),("2CO","2 Corinthians",13,("2 Cor","II Corinthians","Second Corinthians")),
 ("GAL","Galatians",6,("Gal",)),("EPH","Ephesians",6,("Eph",)),("PHP","Philippians",4,("Phil","Php")),("COL","Colossians",4,("Col",)),("1TH","1 Thessalonians",5,("1 Thess","I Thessalonians","First Thessalonians")),
 ("2TH","2 Thessalonians",3,("2 Thess","II Thessalonians","Second Thessalonians")),("1TI","1 Timothy",6,("1 Tim","I Timothy","First Timothy")),("2TI","2 Timothy",4,("2 Tim","II Timothy","Second Timothy")),("TIT","Titus",3,("Tit",)),
 ("PHM","Philemon",1,("Phlm",)),("HEB","Hebrews",13,("Heb",)),("JAS","James",5,("Jas","Jm")),("1PE","1 Peter",5,("1 Pet","I Peter","First Peter")),("2PE","2 Peter",3,("2 Pet","II Peter","Second Peter")),
 ("1JN","1 John",5,("1 Jn","I John","First John")),("2JN","2 John",1,("2 Jn","II John","Second John")),("3JN","3 John",1,("3 Jn","III John","Third John")),("JUD","Jude",1,("Jud",)),("REV","Revelation",22,("Rev","The Revelation",)),
]

BOOKS = tuple(Book(*row) for row in _DATA)


def _key(value: str) -> str:
    value = value.casefold().replace(".", "")
    return re.sub(r"\s+", " ", value).strip()


ALIASES = {_key(alias): book for book in BOOKS for alias in (book.name, *book.aliases)}
ALIASES_BY_LENGTH = sorted(ALIASES, key=len, reverse=True)


def recognize_book(text: str, start: int) -> tuple[Book, int] | None:
    tail = text[start:]
    for alias in ALIASES_BY_LENGTH:
        pattern = re.escape(alias).replace(r"\ ", r"\s*")
        match = re.match(pattern + r"\.?(?![A-Za-z])", tail, re.IGNORECASE)
        if match:
            return ALIASES[alias], start + match.end()
    return None

