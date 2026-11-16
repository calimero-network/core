from enum import Enum


class TeamRole(str, Enum):
    LEAD = "lead"
    MEMBER = "member"
    READONLY = "readOnly"

    def __str__(self) -> str:
        return str(self.value)
