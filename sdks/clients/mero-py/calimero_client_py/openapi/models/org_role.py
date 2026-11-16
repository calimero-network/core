from enum import Enum


class OrgRole(str, Enum):
    ADMIN = "admin"
    GUEST = "guest"
    MEMBER = "member"
    OWNER = "owner"

    def __str__(self) -> str:
        return str(self.value)
