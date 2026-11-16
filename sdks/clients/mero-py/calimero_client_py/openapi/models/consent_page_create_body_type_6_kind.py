from enum import Enum


class ConsentPageCreateBodyType6Kind(str, Enum):
    LEAVEORG = "leaveOrg"

    def __str__(self) -> str:
        return str(self.value)
