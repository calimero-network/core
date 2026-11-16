from enum import Enum


class GovernanceSignUp(str, Enum):
    CLOSED = "closed"
    INVITE = "invite"
    OPEN = "open"

    def __str__(self) -> str:
        return str(self.value)
