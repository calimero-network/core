from enum import Enum


class ConsentPageCreateBodyType5Kind(str, Enum):
    CREATEINVITE = "createInvite"

    def __str__(self) -> str:
        return str(self.value)
