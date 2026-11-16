from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from ..models.consent_page_create_body_type_8_subject_role import (
    ConsentPageCreateBodyType8SubjectRole,
)

T = TypeVar("T", bound="ConsentPageCreateBodyType8Subject")


@_attrs_define
class ConsentPageCreateBodyType8Subject:
    """
    Attributes:
        org (str):
        account (str):
        role (ConsentPageCreateBodyType8SubjectRole):
    """

    org: str
    account: str
    role: ConsentPageCreateBodyType8SubjectRole
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        org = self.org

        account = self.account

        role = self.role.value

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "org": org,
                "account": account,
                "role": role,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        org = d.pop("org")

        account = d.pop("account")

        role = ConsentPageCreateBodyType8SubjectRole(d.pop("role"))

        consent_page_create_body_type_8_subject = cls(
            org=org,
            account=account,
            role=role,
        )

        consent_page_create_body_type_8_subject.additional_properties = d
        return consent_page_create_body_type_8_subject

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
