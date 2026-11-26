from enum import Enum


class AppSecretsSecretsAdditionalPropertySource(str, Enum):
    APP = "app"
    CLUSTER = "cluster"
    DEPLOYMENT = "deployment"

    def __str__(self) -> str:
        return str(self.value)
