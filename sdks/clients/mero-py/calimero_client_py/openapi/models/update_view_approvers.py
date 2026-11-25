from enum import Enum


class UpdateViewApprovers(str, Enum):
    MANAGERS = "managers"
    OPERATOR = "operator"
    OWNER_AND_ADMINS = "owner-and-admins"

    def __str__(self) -> str:
        return str(self.value)
