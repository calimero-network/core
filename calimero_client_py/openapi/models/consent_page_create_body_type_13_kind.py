from enum import Enum


class ConsentPageCreateBodyType13Kind(str, Enum):
    RENAMEORG = "renameOrg"

    def __str__(self) -> str:
        return str(self.value)
