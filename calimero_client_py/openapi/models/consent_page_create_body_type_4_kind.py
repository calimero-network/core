from enum import Enum


class ConsentPageCreateBodyType4Kind(str, Enum):
    CREATEORG = "createOrg"

    def __str__(self) -> str:
        return str(self.value)
