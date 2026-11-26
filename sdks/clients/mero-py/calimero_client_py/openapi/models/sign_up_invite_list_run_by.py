from enum import Enum


class SignUpInviteListRunBy(str, Enum):
    OPERATOR = "operator"
    OWNER = "owner"

    def __str__(self) -> str:
        return str(self.value)
