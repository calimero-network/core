from enum import Enum


class AppUpdateResponse200Approvers(str, Enum):
    MANAGERS = "managers"
    OPERATOR = "operator"
    OWNER_AND_ADMINS = "owner-and-admins"

    def __str__(self) -> str:
        return str(self.value)
