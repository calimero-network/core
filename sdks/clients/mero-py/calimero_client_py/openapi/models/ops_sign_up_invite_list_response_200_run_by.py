from enum import Enum


class OpsSignUpInviteListResponse200RunBy(str, Enum):
    OPERATOR = "operator"
    OWNER = "owner"

    def __str__(self) -> str:
        return str(self.value)
