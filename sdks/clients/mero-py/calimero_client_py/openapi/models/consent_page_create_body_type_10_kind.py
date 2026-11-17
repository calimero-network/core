from enum import Enum


class ConsentPageCreateBodyType10Kind(str, Enum):
    SETMEMBERROLE = "setMemberRole"

    def __str__(self) -> str:
        return str(self.value)
