from enum import Enum


class ConsentPageCreateBodyType14Kind(str, Enum):
    RENAMEORG = "renameOrg"

    def __str__(self) -> str:
        return str(self.value)
