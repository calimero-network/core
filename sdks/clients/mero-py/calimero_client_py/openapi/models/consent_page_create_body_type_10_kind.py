from enum import Enum


class ConsentPageCreateBodyType10Kind(str, Enum):
    REMOVEMEMBERTEAMS = "removeMemberTeams"

    def __str__(self) -> str:
        return str(self.value)
