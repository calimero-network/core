from enum import Enum


class GrantType3Kind(str, Enum):
    ANYONE = "anyone"

    def __str__(self) -> str:
        return str(self.value)
