from enum import Enum


class AccountViewVia(str, Enum):
    APIKEY = "apiKey"
    SESSION = "session"

    def __str__(self) -> str:
        return str(self.value)
