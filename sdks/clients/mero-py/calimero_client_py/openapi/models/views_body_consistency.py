from enum import Enum


class ViewsBodyConsistency(str, Enum):
    STALE = "stale"
    STRONG = "strong"

    def __str__(self) -> str:
        return str(self.value)
