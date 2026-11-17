from enum import Enum


class ConsentPageCreateBodyType7Kind(str, Enum):
    LEAVEORG = "leaveOrg"

    def __str__(self) -> str:
        return str(self.value)
