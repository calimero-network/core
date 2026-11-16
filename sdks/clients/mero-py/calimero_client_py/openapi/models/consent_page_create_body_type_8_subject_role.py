from enum import Enum


class ConsentPageCreateBodyType8SubjectRole(str, Enum):
    ADMIN = "admin"
    GUEST = "guest"
    MEMBER = "member"
    OWNER = "owner"

    def __str__(self) -> str:
        return str(self.value)
