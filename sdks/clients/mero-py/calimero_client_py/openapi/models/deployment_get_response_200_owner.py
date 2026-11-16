from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from ..models.deployment_get_response_200_owner_kind import (
    DeploymentGetResponse200OwnerKind,
)
from typing import cast, Union

T = TypeVar("T", bound="DeploymentGetResponse200Owner")


@_attrs_define
class DeploymentGetResponse200Owner:
    """
    Attributes:
        kind (DeploymentGetResponse200OwnerKind):
        name (Union[None, str]):
    """

    kind: DeploymentGetResponse200OwnerKind
    name: Union[None, str]
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        kind = self.kind.value

        name: Union[None, str]
        name = self.name

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "kind": kind,
                "name": name,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        kind = DeploymentGetResponse200OwnerKind(d.pop("kind"))

        def _parse_name(data: object) -> Union[None, str]:
            if data is None:
                return data
            return cast(Union[None, str], data)

        name = _parse_name(d.pop("name"))

        deployment_get_response_200_owner = cls(
            kind=kind,
            name=name,
        )

        deployment_get_response_200_owner.additional_properties = d
        return deployment_get_response_200_owner

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
