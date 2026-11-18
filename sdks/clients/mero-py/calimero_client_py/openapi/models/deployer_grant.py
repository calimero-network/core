from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from ..types import UNSET, Unset
from typing import Union

T = TypeVar("T", bound="DeployerGrant")


@_attrs_define
class DeployerGrant:
    """A grant to install where deployments are granted: until when (Unix seconds; absent: it does not end), when and by
    whom it was granted.

        Attributes:
            granted_at (int):
            granted_by (str):
            until (Union[Unset, int]):
    """

    granted_at: int
    granted_by: str
    until: Union[Unset, int] = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        granted_at = self.granted_at

        granted_by = self.granted_by

        until = self.until

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "grantedAt": granted_at,
                "grantedBy": granted_by,
            }
        )
        if until is not UNSET:
            field_dict["until"] = until

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        granted_at = d.pop("grantedAt")

        granted_by = d.pop("grantedBy")

        until = d.pop("until", UNSET)

        deployer_grant = cls(
            granted_at=granted_at,
            granted_by=granted_by,
            until=until,
        )

        deployer_grant.additional_properties = d
        return deployer_grant

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
