from enum import Enum


class ConsentPageCreateBodyType4SubjectRole(str, Enum):
    ADMIN = "admin"
    GUEST = "guest"
    MEMBER = "member"

    def __str__(self) -> str:
        return str(self.value)
