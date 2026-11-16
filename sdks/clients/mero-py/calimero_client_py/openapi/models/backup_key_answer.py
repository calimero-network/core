from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from typing import cast
from typing import cast, Union

if TYPE_CHECKING:
    from ..models.backup_key_answer_key_type_0 import BackupKeyAnswerKeyType0
    from ..models.backup_key_answer_holder import BackupKeyAnswerHolder


T = TypeVar("T", bound="BackupKeyAnswer")


@_attrs_define
class BackupKeyAnswer:
    """A holder's backup key: who holds it (`{ kind: "operator" }`, `{ kind: "org", id }` or `{ kind: "account", id }`),
    the X25519 public key the recovery disk seals their apps' backups to (`null` while none is set), and how many apps
    that covers.

        Attributes:
            holder (BackupKeyAnswerHolder):
            key (Union['BackupKeyAnswerKeyType0', None]):
            apps (int):
    """

    holder: "BackupKeyAnswerHolder"
    key: Union["BackupKeyAnswerKeyType0", None]
    apps: int
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        from ..models.backup_key_answer_key_type_0 import BackupKeyAnswerKeyType0
        from ..models.backup_key_answer_holder import BackupKeyAnswerHolder

        holder = self.holder.to_dict()

        key: Union[None, dict[str, Any]]
        if isinstance(self.key, BackupKeyAnswerKeyType0):
            key = self.key.to_dict()
        else:
            key = self.key

        apps = self.apps

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "holder": holder,
                "key": key,
                "apps": apps,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.backup_key_answer_key_type_0 import BackupKeyAnswerKeyType0
        from ..models.backup_key_answer_holder import BackupKeyAnswerHolder

        d = dict(src_dict)
        holder = BackupKeyAnswerHolder.from_dict(d.pop("holder"))

        def _parse_key(data: object) -> Union["BackupKeyAnswerKeyType0", None]:
            if data is None:
                return data
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                key_type_0 = BackupKeyAnswerKeyType0.from_dict(data)

                return key_type_0
            except:  # noqa: E722
                pass
            return cast(Union["BackupKeyAnswerKeyType0", None], data)

        key = _parse_key(d.pop("key"))

        apps = d.pop("apps")

        backup_key_answer = cls(
            holder=holder,
            key=key,
            apps=apps,
        )

        backup_key_answer.additional_properties = d
        return backup_key_answer

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
