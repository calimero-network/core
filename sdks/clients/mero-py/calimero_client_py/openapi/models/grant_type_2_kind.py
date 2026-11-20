from enum import Enum


class GrantType2Kind(str, Enum):
    ACCOUNT = "account"

    def __str__(self) -> str:
        return str(self.value)
