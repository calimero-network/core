from enum import Enum


class OrgMembersResponse200MembersItemPasskeysType1(str, Enum):
    VALUE_0 = "3+"

    def __str__(self) -> str:
        return str(self.value)
