from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from typing import cast

T = TypeVar("T", bound="AppDeleted")


@_attrs_define
class AppDeleted:
    """What `DELETE /apps/{app}` answers: the app, and the members of its unit that went with it.

    Attributes:
        app_id (str):
        deleted (bool):
        members (list[str]):
    """

    app_id: str
    deleted: bool
    members: list[str]
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        app_id = self.app_id

        deleted = self.deleted

        members = self.members

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "appId": app_id,
                "deleted": deleted,
                "members": members,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        app_id = d.pop("appId")

        deleted = d.pop("deleted")

        members = cast(list[str], d.pop("members"))

        app_deleted = cls(
            app_id=app_id,
            deleted=deleted,
            members=members,
        )

        app_deleted.additional_properties = d
        return app_deleted

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
