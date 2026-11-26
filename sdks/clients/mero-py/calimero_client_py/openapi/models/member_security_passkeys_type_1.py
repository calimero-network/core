from enum import Enum


class MemberSecurityPasskeysType1(str, Enum):
    VALUE_0 = "3+"

    def __str__(self) -> str:
        return str(self.value)
