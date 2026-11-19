from enum import Enum


class AccessType5Kind(str, Enum):
    GRANTS = "grants"

    def __str__(self) -> str:
        return str(self.value)
