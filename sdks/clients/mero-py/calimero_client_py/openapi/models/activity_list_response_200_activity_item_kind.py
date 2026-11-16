from enum import Enum


class ActivityListResponse200ActivityItemKind(str, Enum):
    APPROVAL = "approval"
    SIGNIN = "signIn"

    def __str__(self) -> str:
        return str(self.value)
