from enum import Enum


class ConsentPageCreateBodyType5Kind(str, Enum):
    ACCEPTINVITE = "acceptInvite"

    def __str__(self) -> str:
        return str(self.value)
