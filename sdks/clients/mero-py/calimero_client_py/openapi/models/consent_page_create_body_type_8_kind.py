from enum import Enum


class ConsentPageCreateBodyType8Kind(str, Enum):
    SETMEMBERROLE = "setMemberRole"

    def __str__(self) -> str:
        return str(self.value)
