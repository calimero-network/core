from enum import Enum


class TeamJoin(str, Enum):
    EVERYONE = "everyone"
    INVITE = "invite"

    def __str__(self) -> str:
        return str(self.value)
