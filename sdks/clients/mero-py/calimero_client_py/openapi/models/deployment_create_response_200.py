from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from typing import cast
from typing import cast, Union

if TYPE_CHECKING:
    from ..models.deployment_record import DeploymentRecord


T = TypeVar("T", bound="DeploymentCreateResponse200")


@_attrs_define
class DeploymentCreateResponse200:
    """
    Attributes:
        id (str):
        deployment (DeploymentRecord): A deployment as the directory records it: the release it offers by app name
            (`addedAt` Unix seconds), who created it and when (Unix seconds).
        host (Union[None, str]):
    """

    id: str
    deployment: "DeploymentRecord"
    host: Union[None, str]
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        from ..models.deployment_record import DeploymentRecord

        id = self.id

        deployment = self.deployment.to_dict()

        host: Union[None, str]
        host = self.host

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "id": id,
                "deployment": deployment,
                "host": host,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.deployment_record import DeploymentRecord

        d = dict(src_dict)
        id = d.pop("id")

        deployment = DeploymentRecord.from_dict(d.pop("deployment"))

        def _parse_host(data: object) -> Union[None, str]:
            if data is None:
                return data
            return cast(Union[None, str], data)

        host = _parse_host(d.pop("host"))

        deployment_create_response_200 = cls(
            id=id,
            deployment=deployment,
            host=host,
        )

        deployment_create_response_200.additional_properties = d
        return deployment_create_response_200

    @property
    def additional_keys(self) -> list[str]:
        return list(self.additional_properties.keys())

    def __getitem__(self, key: str) -> Any:
        return self.additional_properties[key]

    def __setitem__(self, key: str, value: Any) -> None:
        self.additional_properties[key] = value

    def __delitem__(self, key: str) -> None:
        del self.additional_properties[key]

    def __contains__(self, key: str) -> bool:
        return key in self.additional_properties
