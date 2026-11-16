from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from ..types import UNSET, Unset
from typing import cast
from typing import Union

T = TypeVar("T", bound="ConsentPageCreateBodyType2Subject")


@_attrs_define
class ConsentPageCreateBodyType2Subject:
    """
    Attributes:
        package (Union[Unset, str]):
        prefixes (Union[Unset, list[str]]):
    """

    package: Union[Unset, str] = UNSET
    prefixes: Union[Unset, list[str]] = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        package = self.package

        prefixes: Union[Unset, list[str]] = UNSET
        if not isinstance(self.prefixes, Unset):
            prefixes = self.prefixes

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update({})
        if package is not UNSET:
            field_dict["package"] = package
        if prefixes is not UNSET:
            field_dict["prefixes"] = prefixes

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        package = d.pop("package", UNSET)

        prefixes = cast(list[str], d.pop("prefixes", UNSET))

        consent_page_create_body_type_2_subject = cls(
            package=package,
            prefixes=prefixes,
        )

        consent_page_create_body_type_2_subject.additional_properties = d
        return consent_page_create_body_type_2_subject

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
