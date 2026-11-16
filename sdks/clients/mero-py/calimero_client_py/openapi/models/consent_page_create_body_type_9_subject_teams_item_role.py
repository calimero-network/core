from enum import Enum


class ConsentPageCreateBodyType9SubjectTeamsItemRole(str, Enum):
    LEAD = "lead"
    MEMBER = "member"
    READONLY = "readOnly"

    def __str__(self) -> str:
        return str(self.value)
