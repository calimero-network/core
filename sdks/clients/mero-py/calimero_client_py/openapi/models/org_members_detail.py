from enum import Enum


class OrgMembersDetail(str, Enum):
    SECURITY = "security"

    def __str__(self) -> str:
        return str(self.value)
