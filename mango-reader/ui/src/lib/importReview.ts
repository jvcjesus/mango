export interface ImportBook {series:string;suggestedEdition:string|null;id:number;payloadChapter:number;innerPath:string;suggestedNumber:number|null;state:string;assignedNumber:number|null;assignedKind?:string|null;assignedEdition?:string|null;childChapter:number|null}
export function assignmentFor(book:ImportBook, edits:{series?:string;number?:string|number;kind?:string;edition?:string},ignore=false) {
 const number=edits.number == null || String(edits.number).trim()==='' ? book.suggestedNumber : Number(edits.number);
 return {series:edits.series ?? book.series,kind:edits.kind ?? 'volume',number,edition:edits.edition === undefined ? book.suggestedEdition : edits.edition || null,ignore};
}
export function unambiguousSuggestions(books:ImportBook[]):ImportBook[] {
 const pending=books.filter(b=>b.state==='needs_review'&&b.suggestedNumber!==null);
 const key=(b:ImportBook)=>JSON.stringify([b.series,b.suggestedEdition,b.suggestedNumber]);
 return pending.filter(b=>pending.filter(other=>key(other)===key(b)).length===1);
}
