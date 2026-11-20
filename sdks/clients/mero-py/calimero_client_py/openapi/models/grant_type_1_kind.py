from enum import Enum


class GrantType1Kind(str, Enum):
    TEAM = "team"

    def __str__(self) -> str:
        return str(self.value)
