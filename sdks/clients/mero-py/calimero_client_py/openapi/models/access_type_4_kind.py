from enum import Enum


class AccessType4Kind(str, Enum):
    TEAM = "team"

    def __str__(self) -> str:
        return str(self.value)
