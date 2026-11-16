from enum import Enum


class ConsentPageCreateBodyType12Kind(str, Enum):
    RENAMEORG = "renameOrg"

    def __str__(self) -> str:
        return str(self.value)
