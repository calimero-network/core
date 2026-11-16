from enum import Enum


class ConsentPageCreateBodyType4Kind(str, Enum):
    CREATEINVITE = "createInvite"

    def __str__(self) -> str:
        return str(self.value)
