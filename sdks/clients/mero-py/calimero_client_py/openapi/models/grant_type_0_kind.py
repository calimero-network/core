from enum import Enum


class GrantType0Kind(str, Enum):
    ORG = "org"

    def __str__(self) -> str:
        return str(self.value)
