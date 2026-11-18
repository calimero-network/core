from enum import Enum


class ConsentPageCreateBodyType11Kind(str, Enum):
    SETMEMBERTEAMS = "setMemberTeams"

    def __str__(self) -> str:
        return str(self.value)
