from enum import Enum


class GrantType4Kind(str, Enum):
    LINK = "link"

    def __str__(self) -> str:
        return str(self.value)
