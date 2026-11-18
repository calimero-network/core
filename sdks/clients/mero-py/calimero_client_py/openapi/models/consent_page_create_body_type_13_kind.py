from enum import Enum


class ConsentPageCreateBodyType13Kind(str, Enum):
    REVOKEINVITE = "revokeInvite"

    def __str__(self) -> str:
        return str(self.value)
