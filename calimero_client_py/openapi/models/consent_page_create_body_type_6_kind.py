from enum import Enum


class ConsentPageCreateBodyType6Kind(str, Enum):
    ACCEPTINVITE = "acceptInvite"

    def __str__(self) -> str:
        return str(self.value)
