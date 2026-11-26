from enum import Enum


class UseMode(str, Enum):
    ATTACH = "attach"
    CREATE = "create"

    def __str__(self) -> str:
        return str(self.value)
