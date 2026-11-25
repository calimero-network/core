from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from ..types import UNSET, Unset
from typing import cast
from typing import Union

if TYPE_CHECKING:
    from ..models.secret_declarer import SecretDeclarer


T = TypeVar("T", bound="DeploymentSecretView")


@_attrs_define
class DeploymentSecretView:
    """A secret of a deployment, never its value: the offered apps and releases that declare it, whether a value is set,
    and its fingerprint, setter and time when it is.

        Attributes:
            apps (list[str]):
            declared_by (list['SecretDeclarer']):
            set_ (bool):
            fingerprint (Union[Unset, str]):
            set_by (Union[Unset, str]):
            set_at (Union[Unset, int]):
    """

    apps: list[str]
    declared_by: list["SecretDeclarer"]
    set_: bool
    fingerprint: Union[Unset, str] = UNSET
    set_by: Union[Unset, str] = UNSET
    set_at: Union[Unset, int] = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        from ..models.secret_declarer import SecretDeclarer

        apps = self.apps

        declared_by = []
        for declared_by_item_data in self.declared_by:
            declared_by_item = declared_by_item_data.to_dict()
            declared_by.append(declared_by_item)

        set_ = self.set_

        fingerprint = self.fingerprint

        set_by = self.set_by

        set_at = self.set_at

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "apps": apps,
                "declaredBy": declared_by,
                "set": set_,
            }
        )
        if fingerprint is not UNSET:
            field_dict["fingerprint"] = fingerprint
        if set_by is not UNSET:
            field_dict["setBy"] = set_by
        if set_at is not UNSET:
            field_dict["setAt"] = set_at

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.secret_declarer import SecretDeclarer

        d = dict(src_dict)
        apps = cast(list[str], d.pop("apps"))

        declared_by = []
        _declared_by = d.pop("declaredBy")
        for declared_by_item_data in _declared_by:
            declared_by_item = SecretDeclarer.from_dict(declared_by_item_data)

            declared_by.append(declared_by_item)

        set_ = d.pop("set")

        fingerprint = d.pop("fingerprint", UNSET)

        set_by = d.pop("setBy", UNSET)

        set_at = d.pop("setAt", UNSET)

        deployment_secret_view = cls(
            apps=apps,
            declared_by=declared_by,
            set_=set_,
            fingerprint=fingerprint,
            set_by=set_by,
            set_at=set_at,
        )

        deployment_secret_view.additional_properties = d
        return deployment_secret_view

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
