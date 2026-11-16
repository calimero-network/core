from enum import Enum


class ConsentPageCreateBodyType11Kind(str, Enum):
    REVOKEINVITE = "revokeInvite"

    def __str__(self) -> str:
        return str(self.value)
