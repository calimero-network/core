from enum import Enum


class ConsentPageCreateBodyType10Kind(str, Enum):
    SETMEMBERTEAMS = "setMemberTeams"

    def __str__(self) -> str:
        return str(self.value)
