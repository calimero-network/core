from enum import Enum


class ConsentPageCreateBodyType15Kind(str, Enum):
    DELETEORG = "deleteOrg"

    def __str__(self) -> str:
        return str(self.value)
