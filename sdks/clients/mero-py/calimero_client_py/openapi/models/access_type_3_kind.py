from enum import Enum


class AccessType3Kind(str, Enum):
    ACCOUNTS = "accounts"

    def __str__(self) -> str:
        return str(self.value)
