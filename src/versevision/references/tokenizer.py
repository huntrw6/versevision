from dataclasses import dataclass
import re


@dataclass(frozen=True)
class Token:
    kind: str
    value: str
    start: int
    end: int


_TOKEN = re.compile(r"\s+|\d+|[A-Za-z]+|[:;,&()\[\]-]|.")


def tokenize(text: str, offset: int = 0) -> list[Token]:
    tokens = []
    for match in _TOKEN.finditer(text):
        value = match.group()
        if value.isspace():
            continue
        kind = "NUMBER" if value.isdigit() else "WORD" if value.isalpha() else value
        tokens.append(Token(kind, value, offset + match.start(), offset + match.end()))
    return tokens

