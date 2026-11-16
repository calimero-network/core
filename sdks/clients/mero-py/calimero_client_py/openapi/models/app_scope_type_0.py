from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from ..models.app_scope_type_0_kind import AppScopeType0Kind
from ..types import UNSET, Unset
from typing import Union

T = TypeVar("T", bound="AppScopeType0")


@_attrs_define
class AppScopeType0:
    """
    Attributes:
        kind (AppScopeType0Kind):
        org (str):
        personal (Union[Unset, bool]):
    """

    kind: AppScopeType0Kind
    org: str
    personal: Union[Unset, bool] = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        kind = self.kind.value

        org = self.org

        personal = self.personal

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "kind": kind,
                "org": org,
            }
        )
        if personal is not UNSET:
            field_dict["personal"] = personal

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        kind = AppScopeType0Kind(d.pop("kind"))

        org = d.pop("org")

        personal = d.pop("personal", UNSET)

        app_scope_type_0 = cls(
            kind=kind,
            org=org,
            personal=personal,
        )

        app_scope_type_0.additional_properties = d
        return app_scope_type_0

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
