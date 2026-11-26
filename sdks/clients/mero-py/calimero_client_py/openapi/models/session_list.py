from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from typing import cast

if TYPE_CHECKING:
    from ..models.session_view import SessionView


T = TypeVar("T", bound="SessionList")


@_attrs_define
class SessionList:
    """`GET /auth/sessions`: the account's live sessions, latest active first.

    Attributes:
        sessions (list['SessionView']):
    """

    sessions: list["SessionView"]
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        from ..models.session_view import SessionView

        sessions = []
        for sessions_item_data in self.sessions:
            sessions_item = sessions_item_data.to_dict()
            sessions.append(sessions_item)

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "sessions": sessions,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.session_view import SessionView

        d = dict(src_dict)
        sessions = []
        _sessions = d.pop("sessions")
        for sessions_item_data in _sessions:
            sessions_item = SessionView.from_dict(sessions_item_data)

            sessions.append(sessions_item)

        session_list = cls(
            sessions=sessions,
        )

        session_list.additional_properties = d
        return session_list

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
