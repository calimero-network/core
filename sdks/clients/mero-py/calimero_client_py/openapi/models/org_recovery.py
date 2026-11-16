from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from typing import cast
from typing import cast, Union

if TYPE_CHECKING:
    from ..models.org_recovery_pending_type_0 import OrgRecoveryPendingType0


T = TypeVar("T", bound="OrgRecovery")


@_attrs_define
class OrgRecovery:
    """
    Attributes:
        public_key (str):
        set_at (int):
        pending (Union['OrgRecoveryPendingType0', None]):
    """

    public_key: str
    set_at: int
    pending: Union["OrgRecoveryPendingType0", None]
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        from ..models.org_recovery_pending_type_0 import OrgRecoveryPendingType0

        public_key = self.public_key

        set_at = self.set_at

        pending: Union[None, dict[str, Any]]
        if isinstance(self.pending, OrgRecoveryPendingType0):
            pending = self.pending.to_dict()
        else:
            pending = self.pending

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "publicKey": public_key,
                "setAt": set_at,
                "pending": pending,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.org_recovery_pending_type_0 import OrgRecoveryPendingType0

        d = dict(src_dict)
        public_key = d.pop("publicKey")

        set_at = d.pop("setAt")

        def _parse_pending(data: object) -> Union["OrgRecoveryPendingType0", None]:
            if data is None:
                return data
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                pending_type_0 = OrgRecoveryPendingType0.from_dict(data)

                return pending_type_0
            except:  # noqa: E722
                pass
            return cast(Union["OrgRecoveryPendingType0", None], data)

        pending = _parse_pending(d.pop("pending"))

        org_recovery = cls(
            public_key=public_key,
            set_at=set_at,
            pending=pending,
        )

        org_recovery.additional_properties = d
        return org_recovery

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
